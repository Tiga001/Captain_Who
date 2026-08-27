---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-23
---

# IPC 与协议

本文规定 Renderer、Preload、Electron Main 与 Rust Core 之间的契约归属和扩展方式。进程信任关系见 [Electron Host 与进程架构](./electron-host.md)。

## 契约分层

跨进程接口分为两个 workspace package：

| 包                    | 职责                                                                    | 典型内容                                                                                        |
| --------------------- | ----------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| `@mycopilot/protocol` | 与传输无关的 DTO、schema version、解析器、限制常量和 JSON-RPC method 名 | Agent、Automation、MCP、Skill、Browser、Git、Terminal、workspace files、image generation 等协议 |
| `@mycopilot/host-api` | Electron Renderer 可见的 API 形状和 IPC channel 名                      | `HOST_CHANNELS`、领域 HostApi、`HostInvocationResult`、`getHostApi()`                           |

Main、Preload、Renderer 应导入这两个包，而不是复制字符串或重新声明近似类型。Rust 侧同一 JSON-RPC 契约由 Rust Core/Core Server 类型和 serde 校验实现；涉及双运行时排序、digest 或枚举时，代码注释和测试必须保持字节级一致。

## 调用链

一个完整的 Renderer → Rust Core 调用通常经过：

```text
feature client
  → window.mycopilot.host.<domain>.<method>
  → preload domain bridge
  → HOST_CHANNELS.<domain>.<channel>
  → trusted Main IPC registrar
  → CoreServer typed method
  → newline-delimited JSON-RPC
  → Rust Core parser/handler
```

纯 Electron Main 能力在 registrar 后终止，例如原生对话框、窗口状态、终端、workspace 文件预览和浏览器 surface。Rust Core 业务能力继续通过 `CoreServer` 转发。

反向通知通常经过：

```text
Rust Core notification 或 Main service event
  → Main fan-out 到已登记 Renderer
  → preload parser/event router
  → HostApi on*/subscribe* 回调
  → feature hook/reducer
```

订阅函数必须返回解除订阅回调；组件 effect 必须在 cleanup 中调用。事件消费者仍需检查业务 identity，不能因为事件来自受信 Main 就认为它属于当前页面。

## Channel 真源

所有 Electron channel 名只允许定义在 `packages/host-api/src/channels.ts` 的 `HOST_CHANNELS`。ESLint 禁止在 `src/main` 和 `src/preload` 的生产代码中出现硬编码 `host:` 字面量。

当前领域为：

- `core`
- `app`
- `agent`
- `attachments`
- `automations`
- `browser`
- `git`
- `imageGeneration`
- `mcp`
- `office`
- `resources`
- `search`
- `skills`（Skill 领域的 channel key）
- `storage`
- `terminal`
- `workspaceFiles`

重命名 channel 是 Main/Preload 同步变更，不能保留一侧兼容别名而不写迁移测试。

## Host API 暴露

`src/preload/index.ts` 构造完整 `HostApi`，并仅在 `process.contextIsolated` 时通过 `contextBridge.exposeInMainWorld('mycopilot', { host })` 暴露。否则 Preload 直接失败。

领域较复杂时使用独立 bridge，例如 `AgentIpcBridge`、`AutomationIpcBridge`、`BrowserIpcBridge`、`McpIpcBridge` 和 `TerminalIpcBridge`。简单只读调用可以内联，但仍必须使用 `HOST_CHANNELS` 和共享类型。

Preload 的职责包括：

- 限制可调用方法和参数形状。
- 将 Electron 事件对象截断，只向 Renderer 传 payload。
- 对安全敏感事件执行共享 parser 并 fail closed。
- 对高频/单向传输执行有界处理，例如 terminal 输入按 64 Ki UTF-16 安全边界分片。

Preload 不应执行业务授权、访问 SQLite 或持有跨请求状态；事件 router 仅负责按稳定 identity 分发。

## Main 发送方信任

`createTrustedIpcMain()` 包装所有 `ipcMain.handle` 和 `ipcMain.on`。主窗口创建时登记 WebContents id 与精确入口 URL：

- 事件必须来自 `event.sender.mainFrame`，子 frame 一律拒绝。
- 打包 `file:` 页面必须匹配入口 pathname。
- 开发页面必须匹配登记入口 origin。
- WebContents 销毁后立即移除登记。

不允许某个领域绕过 trusted wrapper 直接注册生产 IPC。单向消息的拒绝只能安全记录 channel，不应打印未受信 payload。

受管 browser guest 不在主 Renderer 信任登记中，因此不能调用 Host API。

## 输入、输出与解析

TypeScript 类型不构成运行时校验。每个不可信边界需要按风险选择解析位置：

- Renderer → Main：Main handler 校验参数，或调用共享严格领域 parser。
- Main → Renderer 事件：Preload 在交给页面前解析，失败时丢弃或抛出安全错误。
- Main ↔ Core Server：`CoreServer` 解析 Rust Core 的 `unknown` 响应和通知；Rust Core 对请求执行 serde/领域校验。
- 持久 JSON → Renderer：Storage client 使用精确 key、长度、枚举和 schema version 校验。

高价值契约应使用“仅允许已知键”的严格 parser。新增字段必须提升相应 schema version 或按协议规定提供兼容解析；不能让调用方静默接受拼写错误和额外权限字段。

## 错误契约

Electron 对 handler 抛出的 Error 通常只可靠保留 message。需要结构化恢复的调用使用：

```ts
type HostInvocationResult<T> =
  { ok: true; value: T } | { ok: false; error: { message: string; code?: number; data?: unknown } }
```

Main 通过 `captureHostInvocation()` 保留 Core Server JSON-RPC 的 code/data，Renderer client 使用 `unwrapHostInvocation()` 还原 `HostInvocationError`。MCP、Skill、图片生成、部分 Agent 与 Artifact 操作依赖这种结构化错误进行 revision refresh、重试或明确的不重试处理。

并非所有历史 Host API 方法都返回 `HostInvocationResult`；简单查询和旧接口仍可能直接 reject。修改既有接口返回形状属于契约变更，需要同步所有消费者和测试，不能只在一侧包裹。

错误 payload 必须是可序列化、有限且对 Renderer 安全的。不得包含凭据、绝对托管路径、网页敏感 URL、CDP/Target identity、子进程环境或原始第三方响应。

## 请求、命令与事件

- `invoke/handle`：需要唯一结果或结构化错误的请求。
- `send/on`：允许丢弃或无需应答的高频命令，例如 terminal 输入与 ACK。
- Main → Renderer event：窗口状态、Agent、MCP changed、Skill changed、Terminal output/exit、Browser surface command。
- Main → Renderer event：Automation event/resync 和原生通知点击产生的导航 intent。
- Main ↔ Core Server JSON-RPC notification：长任务事件、Automation event/resync 和 Managed Playwright 反向桥命令。

选用单向传输不等于不需要校验。必须有 owner、session/Run/request id、序列或 generation，并定义服务退出时如何终止。

## Automation 协议边界

Automation 在 UI 中名为 `Scheduled`，协议、代码和本文均使用 Automation；一次执行称 Automation Run。
业务状态机见 [Scheduled Automation](../subsystems/scheduled-automations.md)。当前共享 DTO 使用：

- `AUTOMATION_SCHEMA_VERSION = 1`；
- `AUTOMATION_PERMISSION_MODE_VERSION = 2`；
- Automation JSON-RPC error code `-32045`，结构化 `data.type = automation`。

这些都是 transport/domain envelope 版本，不是 SQLite schema。当前 canonical SQLite schema 是 v24，由 Rust Core storage 独立校验；Renderer、Preload 和 Main 不读取、协商或转发数据库 schema version。

Renderer-facing `AutomationsHostApi` 暴露十个 request：

```text
list / get / create / update / setEnabled / runNow / delete
listRuns / attentionSummary / acknowledgeAttention
```

它还暴露三个可解除订阅的输入面：`onEvent`、`onResync` 和 `onOpenRequested`。对应 Electron channel 统一定义在 `HOST_CHANNELS.automations`；Preload 只转发固定 invoke/event，并在 listener ready 后发送 `resyncReady` 握手，不提供任意 automation method 或通用 IPC。

Main ↔ Core Server 使用同名的十个 Automation 业务 JSON-RPC method。应用级通知另有独立协议；其中原生投递方法为 **Host-only**：

```text
notifications.claim / validate / acknowledge / release / suppress
notifications.list / summary / markSeen
notifications.settings.get / settings.update
```

claim/validate/acknowledge/release/suppress/list/summary 只供 Main 的 `SystemNotificationCoordinator` 使用，不得加入 Renderer Host API 或 Electron invoke allowlist。Renderer 只获得 settings、原生点击导航，以及用于点击失败补偿的精确 event `markSeen`；没有通知列表或 badge API。claim token、lease、最终 validation 和 ACK/release 都由 Core Server/Rust Core 持久状态校验；Renderer 不是投递授权边界。

Core Server 主动发送两类 notification：

- `automation.event`：包含 `sequence`、`eventId`、kind、`automationId`、可选 `runId`/`resourceRevision`。它是有序失效通知，不携带完整 Automation task/Run 真相。
- `automation.resync`：当前 reason 为 `core_started`，携带冻结的 `lastSequence`。Main 缓存启动 resync，Preload 完成 `resyncReady` 后重放；Renderer 收到后重新读取权威 snapshot。

Main 另以 `host:automation.openRequested` 发送经 parser 校验的 `AutomationOpenRequest`。这是原生通知点击后的导航 intent，只允许打开精确 Automation task 或 Conversation/message；它既不是 Core Server JSON-RPC notification，也不能证明 Automation Run 已成功。

事件正确性依赖 identity 与排序：Renderer 必须丢弃重复/倒序 `sequence`，发现 resync 或不能证明连续性时重新 list/get/listRuns/attention；不能通过 event 文案、到达时间或 `resourceRevision` 猜测缺失状态。update/enable/delete 使用 Automation task revision/CAS，create/runNow 使用稳定 request identity，结构化冲突必须保留到 UI recovery。

## 原生能力

Renderer 不能提交任意路径来替代原生选择：

- MCP executable/cwd、项目目录、头像、附件和 Skill 安装目录由 Main 打开原生 picker。
- Browser Artifact 导出由 Main 打开 save dialog；Renderer 只提交不可变 Artifact reference。
- Browser 自动化文件上传先由 Main picker 选择并准备 process-only 路径能力。

picker 只授予对应操作所需的最小能力。“用户选择了路径”不能自动授权执行、递归读取或后续不同请求复用。

## 新增 Host API 方法的标准步骤

1. 跨 Main/Core Server 或被多个领域复用的非平凡 payload，先在 `packages/protocol` 定义 DTO、schema version、限制与双向 parser；仅属于 Electron Main/Preload 边界的窄类型可放在 `packages/host-api`，但仍要在 Main/Preload 边界做运行时校验。
2. 在 `packages/host-api/src/channels.ts` 增加唯一 channel。
3. 在 `packages/host-api/src/index.ts` 的正确领域接口增加方法；确定直接 reject 或 `HostInvocationResult`。
4. 在独立 Preload bridge 中映射 invoke/send/event，并解析 Main 事件。
5. 在 Main 的领域 registrar 中通过 `TrustedIpcMain` 注册 handler，校验参数并截断输出。
6. 若调用 Rust Core，在 `CoreServer` 增加类型化方法、JSON-RPC method 和响应 parser。
7. Renderer 通过 feature client 使用 Host API，不直接散布 `getHostApi()` 调用。
8. 为 protocol、Preload、Main registrar、Core Server transport、Rust Core handler 和 Renderer recovery 分层测试。
9. 更新本文件、对应子系统文档和必要的兼容/迁移说明。

## 状态与安全不变量

1. Channel 名只有一个真源，Main/Preload 不允许字符串复制。
2. 每个 Main handler 都经过受信发送方校验。
3. Renderer 永远拿不到通用 IPC、Node/Electron 对象或 Core Server 进程管道。
4. 类型化 DTO 在运行时边界仍必须解析；来自 Rust Core/Electron Main 的数据也不能盲信。
5. 权限字段、revision、digest、owner 和 instance identity 只能由权威一侧生成或验证。
6. 绝对托管路径和 process-only capability 不得进入 Renderer、持久会话消息或模型 JSON。
7. 订阅必须可解除，迟到事件必须通过身份和 generation 拒绝。
8. Automation Host-only 通知投递 RPC 永远不进入 Renderer allowlist；`resyncReady` 只声明 listener ready，不授予业务权限。
9. Automation event/resync 不是状态或系统通知送达 receipt；业务消费者必须回读 SQLite 派生的权威 snapshot。
10. Automation DTO schema v1、permission mode v2 和 SQLite schema v24 必须分别命名、分别验证。

## 代码真源

- Electron channel：`packages/host-api/src/channels.ts`
- Renderer Host API 类型与错误：`packages/host-api/src/index.ts`
- 共享协议与 parser：`packages/protocol/src/`
- Automation TypeScript 协议：`packages/protocol/src/automations.ts`
- Automation 跨语言 fixture：`packages/protocol/fixtures/automation-contract-v1.json`
- Automation Rust DTO/method：`crates/protocol-rs/src/automations.rs`、`crates/protocol-rs/src/methods.rs`
- Preload 组合：`src/preload/index.ts`
- Preload 领域桥：`src/preload/*IpcBridge.ts`
- Automation Preload bridge：`src/preload/AutomationIpcBridge.ts`
- Main IPC 组合：`src/main/ipc.ts`
- Main 领域 registrar：`src/main/ipc/*.ts`
- Automation Main registrar：`src/main/ipc/automationIpc.ts`
- 通用原生通知 owner：`src/main/notifications/systemNotificationCoordinator.ts`
- 通知 Main registrar：`src/main/ipc/notificationIpc.ts`
- 发送方信任包装：`src/main/ipc/trustedIpc.ts`
- Renderer Host 客户端：`src/renderer/src/host/hostClient.ts`
- Main/Core Server JSON-RPC：`src/main/core/jsonRpcClient.ts`、`src/main/core/coreServer.ts`
- Rust transport：`crates/core-server/src/transport/`
- Automation Rust transport：`crates/core-server/src/transport/automation_rpc.rs`

## 测试与验证

协议变更至少应覆盖：

- 合法值 round trip。
- 缺字段、额外字段、错误 schema version、越界字符串/数组、非法枚举。
- Main handler 的受信/未受信发送方。
- Preload 事件解析失败时 fail closed。
- Core Server 返回结构化错误时 code/data 能到达 Renderer recovery。
- unsubscribe、WebContents 销毁和迟到通知。
- Automation 的跨语言 method/fixture、Host-only method 隔离、event sequence/resync replay、revision conflict 和原生通知 claim/validate/ACK/release。

常规命令：

```bash
pnpm typecheck
pnpm lint
pnpm test:web
pnpm test:rust
pnpm test:automation-core-e2e
```

Automation 分层测试真源包括 `packages/protocol/src/automations.test.ts`、`src/preload/AutomationIpcBridge.test.ts`、`src/main/core/ipc.automation.test.ts`、`src/main/core/coreServer.automation.test.ts` 和 `src/main/core/automationHostRealCore.integration.test.ts`。通用通知另由 `packages/protocol/src/notifications.test.ts`、`src/preload/NotificationIpcBridge.test.ts`、`src/main/core/ipc.notifications.test.ts` 与 `src/main/core/systemNotificationCoordinator.test.ts` 覆盖。Automation 真实 Core Server 集成测试当前是独立 project，不包含在 `pnpm test:web` 或 `pnpm check` 中。

## 变更检查表

- [ ] DTO、channel、HostApi、Preload、Electron Main、Core Server/Rust Core 和 Renderer consumer 已同步。
- [ ] 输入与输出在正确边界有严格 parser 和预算。
- [ ] handler 使用 `TrustedIpcMain`，没有直接注册旁路。
- [ ] 错误是否需要 code/data 已明确；恢复策略有测试。
- [ ] 原生路径和敏感 identity 没有进入 Renderer payload。
- [ ] 事件有稳定 owner/sequence/generation，并可解除订阅。
- [ ] schema version、Rust/TypeScript 枚举顺序和 digest 规则保持一致。
- [ ] Automation 变更同步核对 DTO schema、permission mode version、Electron channel、十个 Renderer request、四个 Host-only method 和双语言 fixture。
- [ ] Automation event/resync 的 sequence、startup replay、unsubscribe、Renderer ready handshake 和 authoritative reload 均有测试。
- [ ] 对应架构或子系统文档已更新。

## 当前限制

- HostApi 仍同时存在直接 Promise rejection 和 `HostInvocationResult` 两种错误风格。
- Main ↔ Core Server 使用本机进程管道，没有跨版本远程协商；桌面包必须携带匹配的 Main 与 Rust Core。
- 目前没有从 HostApi 自动生成 Main registrar/Preload bridge 的机制，完整性依靠类型检查、测试和检查表。
- 部分早期协议只在 Rust Core 或消费端完成深度校验；修改这些接口时应补齐共享 parser，而不是继续复制验证逻辑。
- Automation 的真实 Core Server E2E 不在默认 `pnpm check` 内，跨层变更必须显式运行专项命令。
- 当前没有由单一 IDL 自动生成 TypeScript/Rust Automation DTO；schema v1、method 和限制依赖 fixture、严格 parser 与双端测试防漂移。
- Main 当前按单主 Renderer 产品形态缓存 startup resync 和最新 pending Automation navigation；多窗口广播 event，但原生通知点击只交付给最近 listener-ready 的受信 Renderer。
