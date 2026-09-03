---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-08-31
---

# Core Server 架构与运行时

本文定义 `crates/core-server` 的当前模块职责、JSON-RPC 运行方式、启动恢复和关停边界。它是维护契约，不是按文件逐行解释。

## 1. 定位与依赖

Core Server（`core-server`）是 Rust Core（`mycopilot-core`）的应用 Host：它把 Agent Runtime、SQLite、MCP、Skill、Git、图片生成和 Electron Host 能力组合成一个可被 Main 调用的进程。它不应成为第二套领域层。

```text
transport
  ├─ 解析/路由 JSON-RPC
  ├─ 请求 admission 与 outbound
  └─ bootstrap/shutdown
       │
       ▼
application
  ├─ Agent use cases / Turn lifecycle
  ├─ Scheduled Automation application / Scheduler
  ├─ shared Notification service / durable batch delivery
  ├─ Multi-Agent service / Dispatcher / Harness / Wait
  ├─ collaboration authorization
  └─ MCP management / policy / persistence
       │
       ├───────────────┐
       ▼               ▼
mycopilot-core       adapters
领域、Runtime、SQLite  Git / Skill / image / MCP runtime
```

允许的方向为 `transport → application → core` 和 `transport/application → adapters → core/protocol`。adapter 不依赖 transport；transport 不直接修改 repository。

## 2. 模块职责

### `application/`

| 模块                          | 职责                                                                                     |
| ----------------------------- | ---------------------------------------------------------------------------------------- |
| `agent`                       | 根/子 Turn 的统一执行、审批、steering、checkpoint、context、usage、command session       |
| `agent_support`               | 对话、工具、world state 等 Host 支持                                                     |
| `agent_collaboration`         | 持久树、Mailbox、模板、消息、结果和协作用例                                              |
| `agent_harness`               | 六个模型协作工具到 application service 的 Host adapter                                   |
| `agent_dispatcher`            | Wake 领取、共享并发 gate、lease、恢复、执行与关停                                        |
| `agent_wait`                  | SQLite 权威的 first-ready wait；进程通知只作加速                                         |
| `collaboration_authorization` | 根 Agent/子 Agent/tree/project 的统一授权与默认配额                                      |
| `automation`                  | Scheduled Automation CRUD/CAS、schedule、权限快照、Scheduler、事件与通知 producer ledger |
| `notification`                | HumanRoot/Automation 共享通知的 list/settings、batch claim、validate、ACK 与投影         |
| `mcp`                         | Registry、审批 envelope、内置能力、Browser 风险、Managed Playwright、管理 RPC 用例       |

生产代码中个别“后续轮次再接线”的旧注释已经不代表状态；是否接线以 `agent_harness.rs` 和 `transport/bootstrap.rs` 的真实组合为准。

### `transport/`

- `bootstrap.rs`：数据根、数据库锁、service 构造、启动对账、通知、Dispatcher、request loop 和关停。
- `request_loop.rs`：逐行 JSON-RPC、请求分类、异步 owner、并发 admission 和 outbound writer。
- `request_handler.rs` 与各 `*_rpc.rs`：参数解码、application 调用、DTO/错误映射。
- `rpc.rs`：安全响应辅助函数。
- `storage_root.rs`：Host 数据根与独立诊断数据库路径边界。

### `adapters/`

当前 adapter 不只有 Git/Image/Skill 三项，还包括：

- `git_dispatcher`；
- `image_generation_dispatcher`；
- `skills_adapter`、`skills_dispatcher`、安装工作流和 source resolution；
- `mcp_runtime`，把 MCP Catalog/调用映射到唯一 Agent ToolRegistry 路径；
- Agent Skill 安装与工作区 Skill 接线。

有阻塞文件或子进程工作的 adapter 必须使用显式 dispatcher/admission，不得在 Tokio request loop 内无界执行。

## 3. JSON-RPC 契约

### Framing 与消息方向

- 一条 UTF-8 JSON 文档占一行；stdout 仅用于协议消息，诊断写 stderr。
- Rust 入站类型要求 `jsonrpc`、string/number `id`、`method`，`params` 可省略。
- Core Server 响应为 `result` 或 `error`。Core Server 主动事件为无 `id` notification。
- Main 的 TypeScript client 可以识别 response 与 notification，并把错误保留为带 `code/data` 的 `CoreJsonRpcError`。

主要错误码：

|        code | 含义                                                      |
| ----------: | --------------------------------------------------------- |
|    `-32700` | JSON 解析失败                                             |
|    `-32602` | 参数或 schema 无效                                        |
|    `-32601` | 方法不存在                                                |
| `-32000` 段 | 领域/Host 错误；部分子系统定义更窄的固定码和 typed `data` |

方法命名空间包括 `core.*`、`agent.*`、`agent.collaboration.*`、`automation.*`、`notifications.*`、`notification.*`、`mcp.*`、`storage.*`、`skills.*`、`git.*`、`office.*`、`imageGeneration.*` 和 `search.*`。完整枚举以 `crates/protocol-rs/src/methods.rs` 为准。Automation 领域错误使用 `-32045` 和 closed typed `data`。`notifications.claim/validate/acknowledge/release/suppress/list/summary` 是 Main 原生投递与点击快照使用的 Host-only 方法；旧 `automation.notifications.*` 仅作兼容，也不应进入 Renderer invoke allowlist。Renderer 只通过窄 Host API 使用 `markSeen`、settings、event/resync 与点击导航；当前没有应用内通知中心。

### 请求 admission

request loop 当前按风险与阻塞特征分流：

| 类别                        |              默认边界 | owner/执行方式                                                                     |
| --------------------------- | --------------------: | ---------------------------------------------------------------------------------- |
| MCP management              |               16 并发 | `McpManagementRequestTracker`，关停停止 admission 并 join/abort                    |
| Browser risk                |               32 并发 | 独立 tracker；关停先取消授权                                                       |
| 大型图片 Artifact 读取      |                2 并发 | 有界 semaphore + 有界 outbound channel，permit 持有至 stdout flush                 |
| Automation/Notification RPC |      无专用 semaphore | Tokio request task 内 `spawn_blocking`；SQLite 串行化，Scheduler 另有独立并发 gate |
| SQLite/历史等 blocking read | 由方法 allowlist 判定 | `spawn_blocking`，避免阻塞 async loop                                              |
| Git/Skill/图片配置写入      |       各自 dispatcher | 显式队列和独立 shutdown                                                            |
| 普通响应/notification       |         无界 outbound | 单一 writer 串行刷 stdout                                                          |

新增方法不能默认落入“普通快速请求”。应先判断它是否阻塞、是否持有大对象、是否可产生副作用，以及谁在 shutdown 时负责已接受任务。

## 4. 启动流程

生产启动按以下阶段进行：

1. 将数据库路径规范化为绝对路径，并取得 exact DB instance lock；锁必须比所有 service/connection 活得更久。
2. 打开 `StorageService`、MCP Registry、内置能力 policy store、图片生成、Skill、Git 和 `AgentService`。
3. 初始化凭据能力。可选凭据后端不可用时相应持久续跑能力 fail closed，但不应泄漏具体秘密错误。
4. 执行启动对账：中断的图片执行、MCP actions/人工票据、FileChange executing audit/Staged commit/delete journal/Run grant、内置能力审批和 orphaned Conversation traces。
5. 连接 MCP Manager/Registry，并挂载内部 `ManagedPlaywrightMcpRuntime` 与 `BrowserRiskCoordinator`。
6. 在任何请求或 Dispatcher 可能写入事件前，分别冻结 collaboration global event cursor、Automation event cursor 与 shared Notification event cursor；连接 notifier，发送 `agent.collaboration.resync`、`automation.resync` 和 `notification.resync`。
7. 启动 collaboration Dispatcher，以及从冻结 cursor 后读取持久事件的 Automation/Notification notifier。两类 channel 都只降延迟，sequence gap 仍从 SQLite 重读。
8. 在最终 MCP-injected `AgentService` 完成启动 reconciliation、且 resync cut 已发布后启动唯一 `AutomationScheduler`。它先回收旧 `admitting` lease，再恢复 `running`/`waiting_for_approval` observer。
9. 启动 Git、Skill、图片配置 dispatcher 和请求 tracker，最后进入 stdin request loop。

这套顺序保证“通知可能丢、持久事实不丢”：重启不依赖上一进程的 channel，也不要求用户再发一个根 Agent Turn 才恢复子 Agent。

## 5. Managed Playwright 反向桥

内部 HostBridge 的方向与普通请求不同：

```text
Rust MCP Manager
  → ManagedPlaywrightHostBridge
  → mcp.builtinPlaywright.command / cancel notification
  → Electron Main 的受管 Browser/CDP broker
  → mcp.builtinPlaywright.dispatchPhase / complete request
  → Rust bridge 一次性 settlement
```

桥最多持有 8 个 pending request，request ID 是 canonical UUIDv4，并单调跟踪 `definitely_not_dispatched`、`possibly_dispatched`、`response_received` 等 certainty。参数和结果不写普通日志。

Managed Playwright 的 Tool execution 预算由 Electron Main 拥有：显式 BrowserRisk 人工等待会暂停但不会重置该预算。Rust reverse bridge 对 `CallTool` 不再叠加 transport deadline，以免等待审批时抢先超时，但仍响应 cancellation/shutdown；其他 bridge operation 继续使用 transport deadline。用户配置的 stdio MCP Server 的 Manager deadline 不受此特例影响。

收到 `core.shutdown` 后，request loop 会在 Managed Playwright runtime 关闭期间继续读取 `complete` 与 `dispatchPhase`，避免因先停止 stdin 导致永远等不到反向完成。Main 当前给整个 Core Server shutdown handshake 6 秒 watchdog；这个值必须与下述各阶段的组合上限一起审查。

## 6. 关停流程

`core.shutdown` 的顺序由 `request_loop.rs` 与 `bootstrap.rs` 共同拥有：

1. request loop 取消 Browser risk，并给已接纳的 Browser risk 请求 1 秒收口。
2. 在 stdin 仍可读时关闭 Managed Playwright runtime；此阶段只处理 `mcp.builtinPlaywright.complete` 与 `mcp.builtinPlaywright.dispatchPhase`。若 stdin EOF，则先关闭精确 bridge 再等待 runtime 结束。
3. request loop 返回后，先停止 Automation 的新 due scan/claim admission。已 admission 的 Automation HumanRoot Turn 和 observer 暂时保留，让后续 Agent shutdown 写出权威 Trace 终态。
4. Core Server 停止 MCP management admission，终止 collaboration notifier 和可选 MCP startup coordinator，关闭短生命周期执行材料 reconciler，并使仅进程内可恢复的 MCP action 失效。Automation 与 shared Notification notifier 此时仍保持工作，以发送关停结算产生的持久事件。
5. Git/Skill/Skill acquisition/图片配置 dispatcher、图片执行、Multi-Agent Dispatcher、活动 Run、已接纳 MCP management 请求和 MCP Manager 并行有界关停；当前显式外层 grace 多为 2 秒。Multi-Agent Dispatcher 的 `shutdown_grace` 默认为 5 秒，算法可先等待一个 grace，再在请求取消后扫描第二个 grace，因此最坏路径可接近 10 秒。
6. Automation Scheduler 做一次最终 bound-Run Trace reconciliation，随后终止本地 observer；真正仍为非终态的 Trace 留给下次启动恢复。完成后才终止 Automation notifier，并在持久通知事件已可重放后终止 Notification notifier。
7. Core Server 最后入队 `core.shutdown` 响应，关闭 outbound admission，并让单一 writer 刷完已接纳消息。超时任务不得伪造成功；持久事实留给下一次启动对账。

EOF 或 request-loop 错误没有 shutdown response，但仍走同一幂等清理路径。新增长期任务必须明确插入上述 owner/admission/drain 顺序。当前 Main 的 6 秒 watchdog 小于 Multi-Agent Dispatcher 的理论最坏收口路径；超时后 Main 会强制停止 Core Server，正确性依赖 SQLite 恢复。修改任一预算时必须同步修正并测试两端。

## 7. Multi-Agent 调度边界

- 根 Agent Human Turn 与子 Agent Wake Turn 共用一个 `AgentTurnConcurrencyGate`，默认全局并发 50。
- Dispatcher 默认每 20 秒续 Wake lease、每 1 秒作 durable fallback scan；单个 shutdown grace 为 5 秒，完整 shutdown 最多可经历两个 grace 阶段。
- claimed 但未 admission 的过期 Wake 可安全重新排队；running 且缺少可恢复 checkpoint 的外部效果不能盲重放，结算为 `outcome_unknown`。
- `AgentWaitKernel` 用 SQLite check/register/recheck 和 50ms durable polling 保证正确性；`Notify` 只降延迟。

完整状态机见 [Multi-Agent 当前架构](multi-agent.md)。

## 8. Scheduled Automation 调度边界

- application 真源是 `application/automation/{service,schedule,scheduler,notifications}.rs`；Renderer 缓存和进程内 `Notify` 都只是加速器。SQLite Task、Run、Event 是 Automation 恢复权威，legacy outbox 只作为 producer/兼容 ledger，原生投递以共享 Notification batch 为准。
- Scheduler 每 30 秒或被唤醒时扫描，due/claim batch 均最多 3，Automation 专用并发为 2；每个 Automation HumanRoot Turn 还必须取得默认上限 4 的进程级 `AgentTurnConcurrencyGate`。
- 持久 Run 内部状态为 `queued → admitting → running ↔ waiting_for_approval → terminal`；协议把内部 `admitting` 投影为 public `starting`，不得在 Renderer 或文档中再暴露内部 lease token。
- claim 的 admission lease 为 60 秒；容量不足或目标 Conversation 忙时回到 `queued` 并退避 5 秒。启动时旧进程留下的所有 `admitting` 都立即回队，不等待旧 lease；多个 missed occurrence 合并为一次 `recovery` Run。
- HumanRoot admission 把 Conversation、message pair、空的 in-progress Trace 与 `admitting → running` 放在同一 `BEGIN IMMEDIATE` 事务中。事务提交后才允许 Provider/MCP 工作；重启后 `running`/`waiting_for_approval` 只从 Trace 和 pending action 恢复。
- 事件 notifier 每 100ms 读取一批最多 256 条 `automation_events`，按全局 sequence 发送 `automation.event`；启动 `automation.resync.lastSequence` 是完整 refetch cut，不是事件内容快照。
- Automation 的 `automation_notification_outbox` 是 producer/兼容 ledger；canonical trigger 把新行原子投影到 HumanRoot/Automation 共用的 `notification_events`/`notification_batches`。Main 原生投递走 `notifications.*`，不是旧 `automation.notifications.*`。

完整 Task/Run 状态机、权限与通知策略见 [Scheduled Automation 子系统](../subsystems/scheduled-automations.md)。

## 9. Main 子进程管理

- `CoreJsonRpcClient` 惰性启动 Core Server；开发模式通过 Cargo 运行 workspace binary，打包模式解析 `process.resourcesPath/core-server[.exe]`。
- Main 冻结 Electron 选择的数据根，删除父环境中任意大小写的 `MYCOPILOT_APP_DATA_ROOT` 和 `MYCOPILOT_STORAGE_DB` 后再安装唯一值。
- 打包模式从 Resources 解析受管组件路径；开发模式可传入受支持的组件 override。
- stdin 写失败只拒绝对应请求；进程 error/exit/显式 stop 会拒绝全部 pending request。
- 通知按 method 分发给进程内订阅者；订阅本身不提供持久 replay。

## 10. 代码真源

- 模块组合：`crates/core-server/src/main.rs`、`crates/core-server/src/application/mod.rs`、`crates/core-server/src/adapters/mod.rs`
- 启动/关停：`crates/core-server/src/transport/bootstrap.rs`
- request admission：`crates/core-server/src/transport/request_loop.rs`
- JSON-RPC 类型和方法：`crates/protocol-rs/src/rpc.rs`、`crates/protocol-rs/src/methods.rs`
- Main client：`src/main/core/jsonRpcClient.ts`、`src/main/core/coreServer.ts`
- Multi-Agent：`crates/core-server/src/application/agent_dispatcher.rs`、`crates/core-server/src/application/agent_harness.rs`、`crates/core-server/src/application/agent_wait.rs`
- Scheduled Automation：`crates/core-server/src/application/automation/`、`crates/core-server/src/application/agent/automation_turn.rs`、`crates/core-server/src/transport/automation_rpc.rs`
- Automation 存储：`crates/core/src/storage/automation_repository.rs`、`crates/core/src/storage/service/automations.rs`
- MCP Host：`crates/core-server/src/application/mcp`、`crates/core-server/src/adapters/mcp_runtime.rs`
- FileChange Host：`crates/core-server/src/application/agent/{approval,pending_action_store}.rs`、`application/agent/action_execution/file_change_authorization.rs`
- Shared Notification：`crates/core-server/src/application/notification.rs`、`transport/notification_rpc.rs`、`crates/core/src/storage/notification_repository.rs`

## 11. 测试

```bash
cargo test -p mycopilot-core-server
cargo test -p mycopilot-core-server --test startup_smoke
cargo test -p mycopilot-core-server --test mcp_stdio_runtime_e2e
cargo test -p mycopilot-protocol-rs
pnpm exec vitest run --project unit packages/protocol/src/notifications.test.ts src/main/core/coreServer.notifications.test.ts src/main/notifications
pnpm exec vitest run --project unit src/main/core
pnpm exec vitest run --project managed-playwright-e2e
pnpm test:automation-core-e2e
```

修改 request loop 或 shutdown 时，至少覆盖：请求分类、tracker admission、EOF、`core.shutdown`、managed bridge completion、outbound drain、Dispatcher recovery、Automation admission/observer reconciliation 和 active Run shutdown。跨语言 DTO 还必须运行 TypeScript fixture/parser 测试。`test:automation-core-e2e` 当前是专项命令，不在 `pnpm test`/`pnpm check` 主链中，发布门禁若要求真实 Core Server Automation 往返必须显式运行。

## 12. 当前限制

- `CoreJsonRpcClient.request` 当前没有通用 per-request timeout；不得用它代替领域 deadline。
- 普通 outbound 是无界 channel；大图片已有独立有界路径，新大响应必须采用同类机制。
- Main 仍重复定义一部分 RPC method 字符串；应以 fixture/双端测试防漂移，并逐步收敛到 protocol package。
- `core-server` 的 public library surface 有意很窄，仅为仓库内 MCP E2E 等 fixture 暴露适配器；它不是通用 SDK。
- 个别源码注释仍保留旧 rollout 轮次描述，不能作为实现状态依据。
- 当前没有多进程横向扩展协议；数据库实例锁要求一个 exact DB 只有一个 Core Server 生命周期 owner。
- Automation 没有可配置并发、Run 总超时或 admission 最大尝试次数；长期等待审批和重复容量退避依赖用户处理或后续状态变化。
- `notifications.*` 的 delivery 子集与 legacy `automation.notifications.*` 的 Host-only 隔离由 Main/Preload invoke allowlist 实现；Core Server stdin 是受信 Main transport，不提供逐请求调用方身份鉴别。
- Main 的 6 秒 shutdown watchdog 与 Multi-Agent Dispatcher 最坏约 10 秒的内部收口预算尚未对齐；超时路径必须按强制终止与启动恢复处理，不能宣称所有 Run 都已优雅结束。

## 13. 变更检查表

- [ ] 模块是否位于正确层，依赖方向是否保持？
- [ ] 新 RPC 是否在 Rust/TypeScript 协议、runtime parser 和 fixture 中同步？
- [ ] 是否为阻塞、大对象、高风险或副作用请求选择了正确 admission？
- [ ] 异步任务是否有明确 owner，并在 shutdown 时停止 admission、取消、等待或保守留待恢复？
- [ ] 错误是否使用稳定 code/data，而非由客户端解析文案？
- [ ] stdout 是否仍只输出单行协议消息，敏感诊断是否被 redaction 后写 stderr？
- [ ] startup reconciliation 是否早于请求 admission，且不依赖进程内通知？
- [ ] Automation 变更是否保持 Task/Run CAS、原子 HumanRoot admission、固定 recovery cut 和 Event/outbox 顺序？
- [ ] 新后台 owner 是否加入 Automation stop-admission、final reconciliation 和 notifier drain 顺序？
- [ ] Notification 变更是否保持 event/batch 持久权威、Host-only delivery RPC 与 resync cursor 顺序？
- [ ] FileChange 恢复是否先对账 audit/transaction/journal/grant，且未知副作用不重放？
- [ ] managed bridge 是否保持 certainty 单调和一次性 settlement？
- [ ] 是否运行 Core Server、协议、Main 和专项恢复测试并更新本文？
