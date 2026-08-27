---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-23
---

# Electron Host 与进程架构

本文描述桌面应用当前的进程拓扑、启动和退出顺序，以及 Electron Host 必须维持的安全边界。Renderer 组织方式见 [前端架构](./frontend.md)，跨进程契约见 [IPC 与协议](./ipc-and-protocol.md)。

## 职责边界

应用由以下运行时组成：

| 运行时                   | 当前职责                                                                                                                                                                                      | 不应承担的职责                                                               |
| ------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| Electron Main            | 冻结应用数据目录；创建窗口；启动和关闭 Core Server；注册受信 IPC；持有原生对话框、原生通知、文件路径、WebContents、Session、浏览器 Target 和 CDP 权限；协调 Automation 通知、终端与浏览器服务 | 不渲染产品 UI；不把原生对象或绝对托管路径交给 Renderer                       |
| Preload                  | 将有限、类型化的 `HostApi` 暴露为 `window.mycopilot.host`；路由 IPC 事件；在部分领域严格解析 Main 事件                                                                                        | 不包含业务状态；不暴露通用 `ipcRenderer`、Node.js 或 Electron API            |
| Renderer                 | React UI、交互状态和视图投影；通过 Host API 请求受信能力                                                                                                                                      | 不直接访问 Node.js、文件系统、PTY、系统凭据、CDP 或 Core Server 标准输入输出 |
| Core Server / Rust Core  | Agent、存储、MCP、Skill、Git Review、图片生成等后端业务；通过 JSON-RPC 与 Main 通信                                                                                                           | 不拥有 Electron 窗口、原生选择器或 WebContents 身份                          |
| Terminal utility process | 运行 `node-pty`、管理 PTY、批处理输出和背压                                                                                                                                                   | 不与 Renderer 直接通信；不决定会话所有权                                     |
| Managed browser guest    | 在专用持久 Session 中显示网页                                                                                                                                                                 | 不继承主 Renderer 权限；不获得 preload、Node.js 或嵌套 webview 能力          |

Main 是操作系统能力与 Web UI 之间的信任边界。Rust Core 是业务权威，但需要原生窗口、文件选择、WebContents 或浏览器 Target 的操作仍由 Main 执行。

## 进程拓扑

```text
React Renderer
    │ window.mycopilot.host
    ▼
context-isolated Preload
    │ Electron IPC
    ▼
Electron Main
    ├── stdin/stdout JSON-RPC ── Rust core-server
    ├── durable Automation outbox lease ── Electron Notification
    ├── Electron message port ── Terminal utility process ── PTY/shell
    └── managed Session/WebContents ── Browser webview guests
                                     └── Main-owned CDP/Playwright bridge
```

Core Server 在开发环境由 `cargo run -p mycopilot-core-server --bin core-server --quiet` 启动；打包环境从 `process.resourcesPath` 启动平台对应的 `core-server` 可执行文件。JSON-RPC 使用一行一个 JSON 消息的 stdin/stdout 管道，stderr 仅作为 Rust Core 诊断输出。

终端服务按需启动：第一个终端会话触发 `utilityProcess.fork()`。所有终端会话共享同一个 utility process，但 Main 使用 utility generation、Renderer WebContents 和 session id 共同约束所有权。

浏览器页面是 Renderer 创建的 `<webview>`，实际 guest、Session、网络策略、Target 和自动化附件均由 Main 管理。详细边界见 [浏览器与自动化](../subsystems/browser-automation.md)。

## Main 启动顺序

模块加载阶段先完成两项不可延后的工作：

1. 读取、创建并 `realpath` Electron `userData`，随后用 `app.setPath('userData', ...)` 冻结该路径。
2. 注册自定义资源 scheme，构造 Core Server 门面、终端桥和 favicon 资源缓存，但尚不创建窗口；Browser Broker、网络 Guard 和 Managed Playwright Host 要到 `app.whenReady()` 后才创建。

`app.whenReady()` 后的当前顺序是：

1. 设置应用名、AppUserModelId 和应用图标。
2. 启动 Core Server 子进程，注册 favicon 协议。
3. 取得 `persist:mycopilot-browser` Session。
4. 创建浏览器 Artifact、文件上传和下载 Broker。
5. 创建网络策略、风险协调器和网络 Guard，并初始化受管 Session。
6. 创建 `BrowserSurfaceManager`、敏感 Target 绑定 Broker 和 Managed Playwright 反向桥。
7. 注册 Host API IPC；所有 handler 都通过受信发送方包装器。Automation registrar 同时启动 Main-owned 原生通知 coordinator，并订阅 Core Server 的 Automation event/resync。
8. 创建主窗口，登记其精确 Renderer 入口，并安装受管 webview 策略。

不要在窗口创建之前调用依赖 `BrowserSurfaceManager` 的组合函数。不要把浏览器 Broker 的初始化移动到 webview 首次导航之后，否则 guest 可能在安全策略完整安装前产生请求。

## 窗口与 Renderer 启动

主窗口采用以下固定安全配置：

- `contextIsolation: true`
- `nodeIntegration: false`
- `sandbox: true`
- `webSecurity: true`
- `webviewTag: true`，但 guest 附加由 Main 的受管策略再次收紧

窗口先以 `show: false` 创建，`ready-to-show` 后才显示。主窗口禁止创建新窗口；外链经 Main 校验后交给系统浏览器。主 frame 只允许留在登记的开发 origin 或打包 `file:` 入口，其他导航会被阻止并按外链处理。

窗口载入后，Renderer 仍有独立的阻塞启动门：`core`、`modelSettings`、`projects`、`uiPreferences`、`composerDrafts`、`conversationMetas` 六个阶段全部 ready 后才可交互。工作区在启动遮罩后保持挂载，但在 ready 前设置 `inert` 和 `aria-hidden`。当前遮罩最短展示 280 ms，阻塞超时 60 s，退出动画 180 ms；重试由新的 startup attempt 隔离迟到响应。

macOS 的普通关闭是 close-to-hide。全屏关闭需要先退出全屏再隐藏；真正退出时 `prepareForQuit()` 关闭该行为。非 macOS 在所有窗口关闭后退出进程。

## 应用级原生通知

原生通知交付由 Main 拥有；HumanRoot 与 Automation 的通知事实、合并批次和投递状态由 Core Server/
Rust Core 的 SQLite 持有。Automation task、Run 与通知策略的完整契约见
[Scheduled Automation](../subsystems/scheduled-automations.md)。完整链路为：

```text
SQLite notification facts / batches
  -> Core Server Host-only claim/validate/acknowledge/release RPC
  -> Main SystemNotificationCoordinator
  -> Electron Notification
  -> trusted, listener-ready Renderer NotificationOpenRequest
```

`notification.event` 和 `notification.resync` 只唤醒 coordinator、降低轮询延迟或提示通知设置重载；它们不是交付真相。Main 每 30 秒主动 drain，并在启动时立即尝试一次；当前单次最多领取 10 个批次、claim lease 60 秒、显示超时 15 秒，原生显示失败后延迟 60 秒重试。

交付必须保持以下顺序：

1. 平台不支持 Electron Notification 时把批次持久结算为 suppressed；Automation attention 仍留在 SQLite。
2. Main claim 后在显示前再次调用 Core Server validate，已删除、已修复或不再满足 policy 的通知不会显示。
3. 只有 Electron 发出 `show` 后才 acknowledge；创建、`show()`、异步 `failed` 或显示超时失败时 release claim。
4. 同一 Main 进程若已经显示但 ACK 暂时失败，只重试 ACK，不重复显示。若进程恰在“系统已显示、ACK 未提交”窗口崩溃，lease 恢复后仍可能重复显示，这是当前原生 API 边界的残余限制。

通知点击不会直接操作 React state。Main 把经过共享 parser 校验的 `NotificationOpenRequest` 交给最近一个已通过 `openRequestedReady` 握手、尚未销毁的受信 Renderer；必要时恢复、显示并聚焦主窗口。Renderer 尚未 ready 时，Main 以 FIFO 暂存最多 32 个请求，溢出时丢弃最早请求。单项点击打开精确 Conversation/message 或 Automation/run；合并通知从用户实际看到且仍有效的快照中，按 Approval、配置 blocked、失败、重要更新、取消、完成的顺序选择最新可导航成员。请求不包含原生 Notification 对象、绝对路径或任意导航 URL。

Automation DTO/notification envelope 使用 schema v1；它与 SQLite canonical schema v23 是两条独立版本线。Main 不解析 SQLite schema，也不把数据库版本暴露给 Renderer。

## 应用数据与环境权威

Electron Main 是应用数据根目录的唯一权威：

- 在 `app.setName()` 可能改变 Electron 路径解析之前冻结 `userData`。
- 启动 Core Server 前大小写不敏感地删除继承的 `MYCOPILOT_APP_DATA_ROOT` 和 `MYCOPILOT_STORAGE_DB`，再注入唯一的 `MYCOPILOT_APP_DATA_ROOT`。
- 删除只允许每次浏览器代理调用使用的内部环境变量，防止其泄漏到长生命周期 Core Server 子进程。
- 开发环境在未显式配置时注入 `.cache` 下的 Office、Word PDF 和 Artifact runtime。
- 打包环境始终把 Office renderer、Word/PDF renderer 和 Artifact Runtime 指向 `process.resourcesPath/components` 下的应用组件，并移除继承的 Artifact Runtime override。OfficeCLI 只有在父环境同时未设置 `MYCOPILOT_OFFICECLI_PATH` 和 `MYCOPILOT_OFFICE_COMPONENTS_DIR` 时才回落到该组件目录；任一显式 override 都会保留，不要把这部分兼容行为误写成全部强制替换。

新增持久目录时应从冻结后的 `appDataRoot` 派生，并明确生命周期、大小上限和清理责任。Renderer 不得接收托管目录的绝对路径。

## 退出顺序

首次收到 `before-quit` 时，Main 阻止默认退出并进入一次性关闭流程：

1. 标记正在退出，立即停止 Automation 原生通知 producer，并让窗口生命周期控制器进入退出模式。停止 producer 必须发生在第一个 `await` 前，避免通知 drain 惰性重启 Core Server。
2. 并行请求终端服务停止和 Core Server graceful shutdown；Automation IPC 其余部分暂时保留，使已接受请求可以收口。
3. Core Server 停止其 Automation Scheduler admission、完成已接纳 Automation Run 的有界关闭，并停止 Managed MCP Manager 后，关闭 Main 的 Managed Playwright 反向桥。
4. 关闭浏览器 surfaces，再释放文件 Broker 和 Artifact Broker。
5. 再次调用 `app.quit()`；`will-quit` 注销 Automation/MCP IPC、图标并执行兜底强制清理。

关键约束是：Core Server shutdown 完成前必须保留同一个 Managed Playwright bridge 和 Browser surface。Rust Core 会发送有界的 close 命令并等待结果；提前拆除 Main 端会把可判定的关闭变成 `outcome_unknown`，并可能遗留附件。

Core Server 的 Main 包装层为 graceful shutdown 设置硬超时，终端 service shutdown 也有独立超时。兜底 `stop()`/`killNow()` 必须保持幂等。

## 状态与安全不变量

1. `userData` 在进程生命周期内不可漂移，Core Server 与 Main 必须使用同一真实根目录。
2. 只有主 frame、精确匹配已登记入口的 Renderer WebContents 可以调用 Host API IPC。
3. Preload 只能暴露 `HostApi`；不得暴露通用 IPC、Node 或 Electron 对象。
4. 主窗口页面和浏览器 guest 是不同信任域。guest 不得获得应用 preload、Node 集成或主窗口 origin。
5. 原生路径、WebContents、Session、Target、CDP 会话、系统凭据和托管 Artifact 路径只存在于受信进程。
6. 所有异步资源都必须有明确 owner、generation/instance 标识和退出清理路径。
7. 开发与打包可以使用不同的可执行文件位置，但不能改变上述权限边界。
8. Renderer 无权 claim、validate、acknowledge 或 release 原生通知；这四个方法仅属于 Main ↔ Core Server 的 Host-only JSON-RPC。
9. Automation event/resync 和本机定时器只能触发重新读取 outbox/快照，不能被当作通知已显示或 Automation Run 已终结的证据。
10. Automation DTO schema v1 与 SQLite schema v23 不得由 Main 合并为单一“自动化版本”。

## 代码真源

- Main 组合与生命周期：`src/main/index.ts`
- 窗口 close-to-hide：`src/main/mainWindowLifecycle.ts`
- Core Server 进程与 JSON-RPC：`src/main/core/jsonRpcClient.ts`
- Core Server 类型化门面：`src/main/core/coreServer.ts`
- Preload 入口：`src/preload/index.ts`
- Automation Main IPC：`src/main/ipc/automationIpc.ts`
- 通用原生通知：`src/main/notifications/systemNotificationCoordinator.ts`
- 通知 IPC：`src/main/ipc/notificationIpc.ts`
- Automation Preload bridge：`src/preload/AutomationIpcBridge.ts`
- Renderer Provider 树：`src/renderer/src/App.tsx`
- Renderer 启动阶段：`src/renderer/src/features/startup/appStartupStages.ts`
- Renderer 启动遮罩：`src/renderer/src/features/startup/AppStartupGate.tsx`
- webview 安全配置：`src/main/webviews/managedWebviewSecurity.ts`
- 终端进程：`src/main/terminal/TerminalBridge.ts`、`src/main/terminal/terminal-service.ts`

## 测试与验证

重点测试包括：

- `src/main/mainWindowLifecycle.test.ts`
- `src/main/core/jsonRpcClient.environment.test.ts`
- `src/main/terminal/TerminalBridge.test.ts`
- `src/main/core/managedWebviewSecurity.test.ts`
- `src/main/core/systemNotificationCoordinator.test.ts`
- `src/main/core/ipc.notifications.test.ts`
- `src/main/core/ipc.automation.test.ts`
- `src/preload/AutomationIpcBridge.test.ts`
- `src/main/core/automationHostRealCore.integration.test.ts`
- `src/renderer/src/app/__tests__/AppStartupGate.browser.test.tsx`
- 浏览器相关测试见 [浏览器与自动化](../subsystems/browser-automation.md)

改动进程组合或启动流程后，至少运行：

```bash
pnpm typecheck
pnpm test:web
pnpm test:automation-core-e2e
```

涉及 Rust 启动、协议或打包资源时，再运行 `pnpm test:rust`、目标平台打包验证以及相应 `verify:*` 脚本。Automation 的真实 Core Server E2E 是独立命令，当前不在 `pnpm test:web`/`pnpm check` 内。

## 变更检查表

- [ ] 新服务的进程所有者、启动点、失败处理和退出顺序已明确。
- [ ] 没有在 Renderer/Preload 暴露 Node、Electron、原生路径或系统凭据。
- [ ] 新 IPC 已按 [IPC 与协议](./ipc-and-protocol.md) 增加 channel、类型、解析和信任检查。
- [ ] 新持久数据从冻结的 `appDataRoot` 派生，并有清理策略。
- [ ] 新 webview/Session 在任何导航前安装权限和网络策略。
- [ ] 异步回调使用 owner、generation、request 或 instance 身份拒绝迟到结果。
- [ ] graceful shutdown 与强制兜底均有测试，且清理函数幂等。
- [ ] Automation notification producer 在 shutdown 的第一个异步等待前停止，Host-only claim/ACK 不会由 Renderer 触发。
- [ ] 原生通知覆盖 unsupported、validate stale、show/failed/timeout、ACK retry、click navigation 和 Renderer 销毁竞态。
- [ ] 开发与打包路径均已验证。

## 当前限制

- 应用当前只维护一个主窗口；信任登记和部分服务组合以此为前提。
- Core Server JSON-RPC 使用进程管道和逐行 JSON，不是可远程访问的服务。
- 终端会话依赖本机 shell 与 `node-pty`，不提供跨应用重启恢复。
- 浏览器只允许受管分区中的 HTTP/HTTPS 页面；网页权限请求统一拒绝。
- Renderer 启动阶段是固定清单，没有通用插件式阶段注册机制。
- Automation 原生通知依赖 Electron/操作系统支持；不支持时只保留 Scheduled attention，不会显示系统通知。
- “系统已显示、Core Server ACK 未提交”的崩溃窗口可能导致一次重复通知，当前没有 OS 级 exactly-once receipt。
- Main 最多保留 32 个 pending notification navigation，溢出时丢弃最早请求；多窗口交付策略仍按最近 listener-ready 的受信 Renderer 处理。
