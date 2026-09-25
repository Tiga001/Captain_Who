---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-08-31
---

# 系统架构总览

本文描述 Captain Who 当前代码的进程边界、依赖方向和权威数据来源。它是架构入口，不替代各子系统的详细设计。

## 1. 系统边界

```text
Renderer（React）
  │ 仅通过类型化 Host API
  ▼
Preload（固定 allowlist）
  │ Electron IPC
  ▼
Main（进程、窗口、Browser、文件选择等 Host 权限）
  │ stdin/stdout，每行一个 JSON-RPC 2.0 消息
  ▼
Core Server（Rust 应用与传输边界）
  ├─ mycopilot-core：Agent Runtime、FileChange、领域模型、SQLite repository
  ├─ mycopilot-mcp-client：MCP Registry/Manager/Catalog/transport
  ├─ AutomationService / AutomationScheduler：持久定时任务与 HumanRoot Turn
  ├─ NotificationService：通用通知事实、合并批次、已读/解决与投递恢复
  └─ adapters：Git、Skill、图片生成、MCP Runtime 等集成
       │
       ├─ SQLite 与受管文件目录
       ├─ 用户 MCP stdio 子进程
       └─ 内部 managed HostBridge → Electron Main → 托管浏览器
```

系统不是单体 Electron 应用：Main 持有操作系统与 Electron 权限，Rust Core 持有 Agent 业务、持久化和多数安全状态机。Renderer 不直接访问 SQLite、Rust Core、任意本地进程或 MCP Server。

## 2. 组件所有权

| 层                                         | 当前职责                                                                             | 不应承担的职责                                     |
| ------------------------------------------ | ------------------------------------------------------------------------------------ | -------------------------------------------------- |
| `src/renderer`                             | 页面、功能状态、interactive/observer 展示、用户操作                                  | 直接读数据库、推断协作真相、启动本地进程           |
| `packages/host-api` + `src/preload`        | Renderer 可见的窄接口与 IPC allowlist                                                | 业务状态机、任意通道转发                           |
| `src/main`                                 | Electron 生命周期、Core Server 子进程、Browser/CDP broker、原生对话框/通知与路径授权 | 复制 Rust 领域模型、把 Renderer 输入当授权事实     |
| `crates/core-server`                       | 用例编排、JSON-RPC、Automation/Agent 调度、启动/恢复/关停、Host adapters             | 复制 Agent Loop 或绕过 `mycopilot-core` repository |
| `crates/core`                              | Agent Runtime、工具契约、权限、检查点、领域服务、SQLite canonical schema             | 依赖 Electron、Renderer 或 JSON-RPC                |
| `crates/mcp-client`                        | MCP 协商、连接、Catalog、调用 certainty、limits、transport                           | 依赖 Agent Runtime、SQLite、Electron               |
| `packages/protocol` / `crates/protocol-rs` | 跨 TypeScript/Rust 的 transport DTO、方法名和 fixture                                | 承载依赖 Rust Core 概念的完整运行时领域模型        |

Renderer 内部依赖继续遵守：

```text
app → features → components → config / host / protocol
```

Core Server 内部依赖继续遵守：

```text
transport → application → core
transport/application → adapters → core/protocol
```

`adapters` 不得依赖 `transport`，transport handler 不得直接写协作表或绕过 application service。

## 3. 通信与协议

### Renderer ↔ Main

- Renderer-facing 通道名由 `packages/host-api` 管理。
- Preload 只暴露固定域和固定方法，不提供通用 IPC escape hatch。
- Main 在跨信任边界处对请求和响应执行 runtime parsing，并重新核验根/子 Agent、Conversation、Project 等身份。

### Main ↔ Core Server

- Core Server 子进程在首次请求时惰性启动；开发模式使用 `cargo run`，打包模式使用 Resources 内的 `core-server` sidecar。
- stdin/stdout 使用 UTF-8 行分隔 JSON-RPC 2.0；一行必须是一条完整请求、响应或通知。
- Core Server 入站请求必须带 string/number `id`。Core Server 发往 Main 的事件使用无 `id` notification。
- 进程退出会拒绝 Main 中全部未完成请求。当前通用客户端没有逐请求超时；具体子系统必须自行拥有 deadline 或 cancellation。
- Managed Playwright 是反向命令桥：Core Server 发 `mcp.builtinPlaywright.command`/`mcp.builtinPlaywright.cancel` notification，Main 回 `mcp.builtinPlaywright.complete`/`mcp.builtinPlaywright.dispatchPhase` 请求。`core.shutdown` 期间 request loop 仍接收后两类收口消息。
- Automation 的任务、Run 和 attention 经普通请求读取；`automation.event`/`automation.resync` 只驱动
  Renderer 失效重读。legacy Automation outbox 只保留 producer ledger/兼容状态，并在同一事务投影到共享
  Notification event；共享 event/batch 才是原生投递真源，由 Main-only RPC 领取和确认。
- FileChange、Notification、Browser data/download 各有独立版本线和窄 DTO；展示 Diff、通知文案、
  下载 display path 都是脱权投影，不能反向构造执行授权或本机绝对路径。

跨语言协议的代码真源是 `crates/protocol-rs/src/methods.rs`、对应 Rust DTO，以及 `packages/protocol` 的 TypeScript DTO/parsers/fixtures。新增或修改协议必须同时更新两端测试；不得只在 Main 中复制字符串常量后宣称协议已统一。

## 4. 数据与恢复真相

- Electron 的 `app.getPath('userData')` 是正式应用的数据根；Main 通过 `MYCOPILOT_APP_DATA_ROOT` 把该能力显式交给 Core Server，并移除父环境中的数据库重定向。
- `storage.sqlite` 是 Conversation、Agent、模板与项目分配、Mailbox、Wake、Approval、FileChange
  audit/run grant、Automation task/Run/event、Notification fact/batch、Browser history/preferences/download
  等持久事实来源。
- 当前 canonical schema 为 **v55**；版本与 catalog fingerprint 的唯一真源是 `crates/core/src/storage/migrations.rs`。v55 增加工作流启用状态；v54 增加工作流定义表；v53 新增按实际任务派发者归属的活动明细；v52 移除协作正文的固定字节上限；v51 为协作活动记录直属父会话及其消息/Trace 位置；v50 保存文件夹引用，v49 允许纯附件引导，v48 为历史搜索增加身份索引。
- 内存 channel、`Notify`、Renderer store 和 notification 只用于降延迟或失效通知。间隙、重启和丢通知必须从 SQLite snapshot/event log 恢复。
- 空库原子创建 v55；exact v54 保留定义增加默认关闭的工作流开关；exact v53 保留历史增加工作流定义表；exact v52 保留历史新增活动明细且不回填旧活动；exact v51 原样保留历史重建 Mailbox 正文约束后依次升 v52、v53、v54、v55。exact v50 仍须协作事件日志为空，依次升级至 v51、v52、v53、v54、v55。不转换旧协作活动；仍有旧事件时返回 `development_storage_schema_reset_required`，由用户先清理历史或显式开发重建。v49 及更早版本、未知 catalog 或外键不匹配也拒绝升级，启动时不自动清空数据库。

## 5. 启动与关停概览

启动顺序的关键不变量是“先建立权威状态与恢复，再接收请求”：

1. 解析并规范化数据根，取得数据库实例锁。
2. 打开 canonical storage、MCP Registry、凭据与各 application service。
3. 执行图片、MCP action、孤立 trace、Approval、FileChange 和 Notification 等启动对账。
4. 冻结 collaboration、Automation 与 Notification event cursor，连接 outbound，发布各自 resync。
5. 启动唯一进程级 Multi-Agent Dispatcher，并恢复 Automation admission lease、Run observer 与到期任务。
6. 启动 Automation Scheduler，使 queued/recoverable Run 可在无 Renderer 操作时继续执行。
7. 创建其余有界 dispatchers/request trackers，最后进入 JSON-RPC request loop。

关停先停止接收新工作，再收口反向浏览器桥、MCP 请求、各 dispatcher、活动 Run、MCP Manager 和 outbound writer。详细时序见 [Core Server 架构](core-server.md)。

## 6. 安全与权限原则

- Renderer 或模型提交的 sender、根 Agent、task path、文件路径、MCP trust 等字段都不是授权事实。
- Host 从 SQLite、当前 Turn context、原生选择器或受管 broker 构造可信 capability。
- 可能已经产生外部副作用的 MCP、Tool 或 Agent Wake 失败不得自动重放；不确定时落为 `outcome_unknown`。
- 敏感参数、MCP 原始参数/结果和凭据不进入普通日志、Renderer state 或协作事件。
- 用户 MCP Server 的外部 transport 当前仅 stdio；HostBridge 是编译期允许的内部托管通道，不是用户可配置的网络入口。
- 子 Agent Conversation 对用户只读；用户只在根 Agent Conversation 发起交互或处理投影后的审批。
- Automation 是无人值守入口：权限在保存时冻结，执行前只允许当前设置撤销，不能因配置变化自动扩权；
  后台 Approval 仍必须由用户显式处理。
- FileChange 的读取 Observation、私有 binding 和 Run grant 才参与执行授权；Renderer Diff、Trace、
  Archive 和 Git 相对展示路径都不能授权写入。
- Browser Download 的真实保存路径留在 Core Server/Main 信任边界，Renderer 和模型只接收安全引用与
  display path；系统通知同样只投递受限 subject，不能携带完整结果、secret 或审批 payload。

## 7. 相关文档

- [Core Server 架构](core-server.md)
- [Multi-Agent 当前架构](multi-agent.md)
- [MCP 子系统](../subsystems/mcp.md)
- [FileChange](../subsystems/file-change.md)
- [浏览器与自动化](../subsystems/browser-automation.md)
- [Scheduled Automation](../subsystems/scheduled-automations.md)
- [通用通知](../subsystems/notifications.md)
- [测试策略](../development/testing.md)
- [构建与发布](../development/build-and-release.md)
- [运行时组件](../development/runtime-components.md)
- [恢复 Runbook](../operations/recovery-runbook.md)

## 8. 代码真源

- 进程与 Core Server client：`src/main/core/jsonRpcClient.ts`、`src/main/core/coreServer.ts`
- Core Server 组合入口：`crates/core-server/src/main.rs`
- 启动/关停：`crates/core-server/src/transport/bootstrap.rs`
- 请求调度：`crates/core-server/src/transport/request_loop.rs`
- Rust JSON-RPC 与方法名：`crates/protocol-rs/src/rpc.rs`、`crates/protocol-rs/src/methods.rs`
- TypeScript 协议：`packages/protocol/src`
- canonical schema：`crates/core/src/storage/canonical_schema.sql`、`crates/core/src/storage/migrations.rs`
- Automation：`crates/core-server/src/application/automation`、`crates/core/src/storage/automation_repository.rs`
- FileChange：`crates/core/src/file_change/`、`crates/core/src/tools/apply_patch.rs`
- Notification：`crates/core-server/src/application/notification.rs`、`crates/core/src/storage/notification_repository.rs`
- Browser data/download：`src/main/browser/`、`crates/core/src/storage/browser_data_repository.rs`、
  `crates/core/src/storage/browser_download_repository.rs`
- 打包边界：`package.json`、`electron-builder.yml`

文档中的版本和限额是便于阅读的快照。代码常量、锁定 manifest 和可执行 gate 与本文冲突时，应先停止发布并更新实现或本文，不能静默选择一方。

## 9. 测试

最低架构验证：

```bash
pnpm format:check
pnpm check:docs
pnpm check:public-docs
pnpm lint
pnpm typecheck
pnpm lint:rust
pnpm test:rust
pnpm test:web
```

协议边界还应运行 `cargo test -p mycopilot-protocol-rs` 和 `packages/protocol/src` 下的 Vitest 契约测试。
修改 Automation Host 全链路时另运行 `pnpm test:automation-core-e2e`；它当前不在 `pnpm check`。完整
测试层级和专项门禁见 [测试策略](../development/testing.md)。

## 10. 当前限制

- 仓库已有 Linux/macOS 源码测试 workflow，但它不执行 Windows 测试、目标平台 package、真实签名、notarization 或完整 release acceptance。
- Main 端仍存在部分重复的 Core Server RPC 字符串常量；在完成统一前，跨语言 fixture 和双方测试是必要防漂移措施。
- 通用 `CoreJsonRpcClient.request` 没有默认逐请求 timeout，长操作依赖子系统 deadline 和进程退出收口。
- 打包、真实签名、专项 Multi-Agent gate 和 Managed Playwright release gate 不在 `pnpm check` 内；内部文档、公开文档和 Agent 头像检查已纳入 `pnpm check`。
- Automation 真实 Core Server E2E 也不在 `pnpm check`，且尚无定时触发到操作系统通知点击的
  packaged E2E。
- 仅 exact v47 支持保留历史的 v48 搜索索引事务升级；v46 及更早版本仍要求显式 reset。

## 11. 变更检查表

- [ ] 新边界是否明确了唯一 owner，且依赖方向未反转？
- [ ] 新跨进程方法是否同时更新 Rust/TypeScript 常量、DTO、parser 和 fixture？
- [ ] 新持久事实是否进入 canonical schema、版本/fingerprint 和 reset 测试？
- [ ] 新异步任务是否有 admission 上限、生命周期 owner、取消与有界关停？
- [ ] 新通知是否仍是 invalidation，而非第二份状态真相？
- [ ] 新 FileChange operation 是否保持 Observation、私有 binding、审计、冲突与恢复边界？
- [ ] 新原生通知是否由持久 fact/batch 驱动，并在显示前重新校验？
- [ ] 新后台调度是否定义 occurrence 幂等、配置 snapshot、lease、恢复和原生通知 outbox？
- [ ] 新外部副作用是否定义 dispatch certainty、幂等与 `outcome_unknown`？
- [ ] 新运行时依赖是否被锁定、校验、打包并加入第三方声明？
- [ ] 是否更新相关当前态文档、测试矩阵和发布门禁？
