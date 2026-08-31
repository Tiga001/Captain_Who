---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-31
---

# 内置终端

内置终端在右侧栏中提供真实 PTY 会话。Renderer 使用 xterm 展示，Electron Main 维护会话所有权，独立 utility process 运行 `node-pty`。右侧栏保活策略见 [右侧栏平台](./right-sidebar.md)，IPC 通用规则见 [IPC 与协议](../architecture/ipc-and-protocol.md)。

## 职责边界

| 层                    | 当前职责                                                                                                                  |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| Renderer              | 创建 xterm、发送输入、订阅输出/退出、ACK 已完成解析的 batch、根据可见尺寸 resize、展示状态                                |
| Preload               | 按 session id 路由 output/exit；将输入按 64 Ki code-unit 分片且不切断 surrogate pair                                      |
| Main `TerminalBridge` | 启动 utility process；绑定 session 与 Renderer WebContents；处理 request timeout、child generation、事件转发和 owner 清理 |
| Terminal utility      | 选择 shell/cwd、创建 PTY、批处理输出、执行背压和 exit drain                                                               |
| PTY/shell             | 执行用户输入，产生终端控制序列和进程退出状态                                                                              |

Terminal 与 Agent 的 `run_command` Tool/managed command session 是不同子系统。右侧栏 PTY 不写入 Agent trace，也不能被模型隐式控制。

## 会话创建

Terminal page 固定到创建时 workspace，初始 cwd 为该 workspace path。Renderer 创建形如 `terminal-<time>-<random>` 的 session id，并先注册事件订阅，再调用 `createSession`，从而不丢失快速启动输出。

Main 校验 session id 必须匹配 `[A-Za-z0-9._:-]{1,160}`，确保全局不重复，并把 session 临时绑定到：

- 精确 Renderer `WebContents.id`
- 当前 `WebContents` 对象
- 当前 terminal utility generation
- create request id

utility process 按需启动。默认 cwd 是请求值，否则使用 `HOME`/用户主目录；默认尺寸为 80×24。它调用 `node-pty.spawn()` 启动平台默认 shell，设置 `TERM=xterm-256color`，并返回 shell、cwd、rows/cols 和 pid 快照。

如果 Renderer 在创建响应前销毁或切换 main-frame，Main 会拒绝请求并向 utility 发送 dispose，迟到成功不能重新建立所有权。

## 输入与 resize

输入使用单向 IPC，以降低按键延迟。Preload 将长字符串按最多 64 Ki UTF-16 code units 分片；若边界落在高位 surrogate 后，会向前移动一个 code unit，避免破坏 Unicode 字符。Main 只转发属于调用 WebContents 的有效 session。

Resize 使用 request/response：xterm `FitAddon` 先计算 rows/cols，只有尺寸实际变化时才调用 Host API。当前仅在容器至少 220×120 px 且可见时 fit；激活页在下一帧和 140 ms 布局稳定后重试，`ResizeObserver` 也以 140 ms settle 触发。

后台 Terminal 保持 PTY，但关闭 cursor blink、blur xterm，并停止不可见容器上的 fit。

## 输出顺序与背压

utility 中的 `TerminalOutputFlowController` 合并 PTY 数据：

- 最多等待 12 ms，或累计到 64 KiB 后发送 batch。
- 每个 session 的 `sequence` 从 1 严格递增。
- 记录已发送但尚未 ACK 的 UTF-8 字节数。
- 达到 1 MiB high watermark 时 pause PTY；累计 ACK 后降到 256 KiB low watermark 才 resume。
- ACK 是累计确认，因为 utility→Main 和 Renderer→utility 的通道均保持同 session 顺序。

Renderer 的 `TerminalOutputWriter` 不在收到 IPC 时立即 ACK，而是在 xterm 的异步 parser 完成 `terminal.write(..., callback)` 后确认。它要求精确 session id 和有效 sequence，最多缓存 256 个乱序 batch；重复 batch 可幂等处理，越界、缺口或 final sequence 矛盾会终止会话并报告协议错误。

该设计保证“Main 已发送”不被误当成“xterm 已消费”，从而将内存背压传回 PTY。

## 退出与最后输出

`TerminalExitEvent.finalOutputSequence` 指明 exit 之前最后一个 output batch。Renderer 必须等 `TerminalOutputWriter` 消费到该序列后再显示退出消息。

Unix 的 node-pty process exit 与 socket close 独立，最后输出可能在 `onExit` 后到达。utility 的 `TerminalExitDrainController` 会等待：

1. PTY stream 已关闭（Unix）；
2. 输出不再处于 pause；
3. 20 ms quiet period 内没有新数据。

最多等待 2 秒，防止坏掉的 stream 或不 ACK 的 Renderer 永久保留 session。Windows 的 ConPTY exit 已在 socket flush 路径之后，不等待独立 close subscription。

显式 kill 会立即删除 utility session、kill PTY，并发送 `signal: 'killed'` 和当前 final sequence。terminal service 崩溃时 Main 为该 generation 的所有 session 发送 `signal: 'terminal-service-exit'`。

## 所有权与清理

Main 只接受 session owner 的 write、resize、kill 和 ACK。以下任一事件都会释放该 WebContents 的全部 session：

- WebContents `destroyed`
- 非 same-document 的 main-frame navigation
- `render-process-gone`

右侧栏 Terminal page 使用 `keep-alive`，切换标签不会卸载 xterm/PTY；关闭页面或 Renderer 卸载会 unsubscribe、dispose output writer、kill Electron Main 持有的 PTY session 并 dispose xterm。

应用退出时 Main 先请求 utility 的 `terminal.shutdown`，超时为 1 秒；普通 service request 默认超时 5 秒。失败后 `killNow()` 强制终止 child。child generation 防止旧 service 响应或事件落入重启后的会话。

## 状态与安全不变量

1. session id 不是权限；每个操作都必须匹配当前 WebContents owner 和 child generation。
2. Renderer 只收到自身 session 的事件。
3. Output sequence 单调且 exit 的 final sequence 必须可完全覆盖。
4. 只有 xterm 完成解析后才能 ACK；不能在 Preload/Main 收到时提前 ACK。
5. 达到 high watermark 必须 pause PTY，低于 low watermark 才 resume。
6. WebContents 销毁、导航、崩溃和应用退出都必须终止其 PTY。
7. 终端 stderr/错误日志不得输出完整环境或敏感诊断报告。

## 代码真源

- 共享协议：`packages/protocol/src/terminal.ts`
- Host API：`packages/host-api/src/index.ts` 的 `TerminalHostApi`
- Preload：`src/preload/TerminalIpcBridge.ts`、`src/preload/TerminalEventRouter.ts`
- Main IPC：`src/main/ipc/terminalIpc.ts`
- Main owner/child bridge：`src/main/terminal/TerminalBridge.ts`
- utility transport：`src/main/terminal/terminalTransportProtocol.ts`
- PTY service：`src/main/terminal/terminal-service.ts`
- 输出背压：`src/main/terminal/TerminalOutputFlowController.ts`
- 退出 drain：`src/main/terminal/TerminalExitDrainController.ts`
- Renderer lifecycle：`src/renderer/src/features/terminal/useTerminalSession.ts`
- Renderer writer：`src/renderer/src/features/terminal/TerminalOutputWriter.ts`
- 右侧栏模块定义：`src/renderer/src/features/rightSidebar/rightSidebarModules.tsx`

## 测试与变更检查表

关键测试：

- `src/main/terminal/TerminalBridge.test.ts`
- `src/main/terminal/TerminalOutputFlowController.test.ts`
- `src/main/terminal/TerminalExitDrainController.test.ts`
- `src/preload/TerminalEventRouter.test.ts`
- `src/renderer/src/features/terminal/__tests__/TerminalOutputWriter.test.ts`
- `src/renderer/src/features/rightSidebar/__tests__/RightSidebarWorkspaceLifecycle.browser.test.tsx`

检查项：

- [ ] create 前已订阅事件，失败和迟到成功均释放 session。
- [ ] 新事件字段在 protocol、Preload、Main 和 Renderer 同步校验。
- [ ] owner、generation、sequence 和 final sequence 的异常路径有测试。
- [ ] 输入分片不破坏 surrogate pair，输出预算按字节计算。
- [ ] pause/resume、exit drain 和 Renderer 不 ACK 情况有界。
- [ ] 页面切换保活，页面关闭/WebContents 异常终止 PTY。
- [ ] Linux/macOS PTY close 与 Windows ConPTY 行为均保持。

## 当前限制

- 会话不跨 Renderer reload 或应用重启恢复。
- Terminal page 创建后固定到当时 workspace；切换项目不会改变 cwd。
- 所有页面共享一个 utility process，service 崩溃会终止该 generation 的全部终端。
- 当前 UI 没有重新启动已退出 session 的原位按钮；需要关闭并新建页面。
- xterm scrollback 当前固定为 8,000 行。
