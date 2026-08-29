---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-08-23
---

# 恢复与故障处理 Runbook

本文用于开发和维护环境中的 Core Server、SQLite、Multi-Agent、MCP 与 Managed Playwright 故障。优先原则是保护持久事实和外部副作用，不通过手工改库“修绿”。

## 1. 先确定故障域

| 现象                                                                 | 首要检查                                                                              | 不要先做                                      |
| -------------------------------------------------------------------- | ------------------------------------------------------------------------------------- | --------------------------------------------- |
| Core Server 无法启动，含 `development_storage_schema_reset_required` | schema version/fingerprint、数据根、完整错误码                                        | 手工改 `PRAGMA user_version` 或删表           |
| 提示 exact DB 已被占用                                               | 是否有 MyCopilot/Core Server 进程仍运行；实例锁路径                                   | 删除 lock file 后继续运行两个 Core Server     |
| 子 Agent 长时间 queued/claimed                                       | Core Server 是否重启、Dispatcher 是否运行、lease deadline、数据库可写                 | 重复 spawn 同一任务或直接把 Wake 改 completed |
| 子 Agent 显示 running，但进程已崩溃                                  | trace/checkpoint/pending action/lease                                                 | 盲重放可能已有外部副作用的 Turn               |
| 子 Agent 等待审批                                                    | 根 Agent Approval projection、原 pending action、expiry                               | 从子 Agent 页面直接写 decision                |
| UI 丢活动或串线                                                      | 根 Agent ID、event sequence、resync、重新 hydrate                                     | 从 notification 时间戳重建状态                |
| 用户 MCP Server 无法启动                                             | Registry status、launch authorization、program/cwd identity                           | 重用旧授权或把 Token 填入 argv                |
| Managed Playwright 卡住                                              | bridge pending、dispatch certainty、Browser surface/target、Core Server/Main shutdown | 开 remote-debugging 后注入完成结果            |

处理前记录：commit SHA、OS/arch、开发或打包模式、数据根、Core Server stderr 的脱敏错误码、触发步骤和最后一次成功命令。不要记录 API Token、MCP 原始参数/结果、Cookie 或审批 ciphertext。

## 2. 通用安全步骤

1. 停止新的用户操作和 release gate；不要让第二个 Core Server 指向同一数据库。
2. 优先正常退出应用，使 `core.shutdown`、Dispatcher、MCP Manager 和 outbound writer 有机会收口。
3. 若进程已退出，保留原 `storage.sqlite`、`storage-backups/` 和相关日志；不要立即覆盖。
4. 区分“可从 SQLite 自动恢复”和“需要显式开发 reset/人工确认”的故障。
5. 在副本或仓库 fixture 上复现。不要让测试命令打开或迁移真实用户数据库。

## 3. Schema/reset-required

### 判断

当前唯一可接受基线为 **schema v27 + exact catalog fingerprint + valid foreign keys**。真源：

```text
crates/core/src/storage/migrations.rs
crates/core/src/storage/canonical_schema.sql
```

下列情况都返回 `development_storage_schema_reset_required`：旧版本、非空未版本化库、当前版本但 catalog 漂移、外键违规。开发策略是不原地迁移，也不自动重写源库。

### 预检

完全退出 MyCopilot 后运行：

```bash
pnpm storage:reset-dev
```

默认是只读 dry-run。它通过 Electron 解析与应用相同的 `userData`，取得 exact database instance lock，并只输出路径和计数。若提示数据库仍被占用，返回应用/进程排查；不要删除 lock 文件绕过 owner。

### 确认重建

阅读 dry-run 摘要后显式执行：

```bash
pnpm storage:reset-dev -- --confirm-reset
```

确认流程：

1. 在 `storage-backups/` 创建权限受限、时间戳命名的 verified SQLite snapshot。
2. 对当前或紧邻 schema，从只读源或临时副本提取 allowlisted configuration；MCP 精确 identity/authorization 也要重新验证。更旧的 schema 不解码配置，报告会明确显示使用默认配置。
3. 在 staging 文件创建 fresh v27 canonical DB。
4. 通过当前 service 写路径恢复配置。
5. 重开生产 storage，核对记录数、`PRAGMA quick_check` 和 `foreign_key_check`。
6. 原子发布新数据库；失败时保留原数据库与恢复备份。

保留内容：

- model/provider settings（含当前代码仍以本机 SQLite 明文保存的模型 Token/Tavily key）；
- UI preferences、Agent prompt preferences；
- 浏览器链接打开位置和下载设置（仅在源 schema 支持对应表时）；
- Skill enablement overrides；
- 默认 image-generation profile；
- MCP Registry、model namespace、仍有效的 launch authorization 与 enabled/trust 状态。

不恢复：Conversation、Project、message、draft、Usage、Approval、Continuation、Compaction、Fork、Agent tree/Mailbox/Wake，以及 Scheduled Automation task、Run、attention、event 与 notification outbox 等运行/会话派生状态。附件、已安装 Skill、生成图片和 credential 目录不在 reset 事务中移动；没有数据库引用的附件可在后续正常启动时被孤儿清理。

### 备份处理

- reset 失败时，优先保留原库；backup 是恢复/取证副本，不应被 reset 检查过程修改。
- 不要直接把一个旧 schema backup 覆盖回运行路径并期待 v27 接受；旧库仍会触发 reset-required。
- 如必须人工还原文件，先停止所有 Core Server、再次复制保存当前文件、在隔离位置验证 SQLite 完整性和 schema，再决定是否替换。仓库当前没有受支持的一键 backup restore 命令。
- 不要把包含本机模型 Token/搜索 key 的 backup 上传到 issue、CI artifact 或公共对象存储。

## 4. Multi-Agent 自动恢复

SQLite 是 Wake、Turn、Mailbox、receipt、Approval 和 event 的恢复真相。正常情况下，只需重新启动同一数据根的 Core Server：bootstrap 会在接收请求前启动 Dispatcher，并立即/周期扫描 queued 与 recoverable Wake。

### Wake 分类

| 持久状态                                      | 恢复动作                                                          |
| --------------------------------------------- | ----------------------------------------------------------------- |
| `queued`                                      | 按 FIFO 等待 Dispatcher claim                                     |
| 过期 `claimed`，无 Run identity               | 安全重新排队/重新 claim；确定的 admission 前失败可结算 failed     |
| `running`，已有 terminal trace                | 观察 terminal fact，完成 result outbox/owner 释放，不重跑 Runtime |
| `waiting_for_approval` 且 pending action 存在 | 保持等待；根 Agent UI 从 durable projection 恢复                  |
| `running` 且有合法 checkpoint                 | 按原 identity/permissions/capability snapshot 恢复                |
| `running` 且无可证明安全的 checkpoint         | 结算 `outcome_unknown`，不重放 Tool/Provider 副作用               |

Wake lease 为 60 秒，Dispatcher 每 20 秒续租、每 1 秒 durable fallback scan。新 Core Server 启动时旧 lease 尚未过期属于正常窗口；周期扫描会在 deadline 后处理，不需要再次重启。

### `outcome_unknown`

`outcome_unknown` 表示系统无法证明外部动作未发生，也没有权威响应。处理方式：

1. 保留原 Run/Wake/trace 与错误分类。
2. 在外部系统检查是否已发生操作，例如文件、远端 MCP、命令或第三方 API 状态。
3. 不直接 retry 原 tool call。若业务上需要补偿或重新执行，由用户创建新的显式任务，并说明已核验的外部状态。
4. 若可建立确定性恢复协议，应作为实现变更加入 receipt/checkpoint/fault-injection，而不是在 Runbook 中约定手工改状态。

### Approval

- 子 Agent Approval 的 source of truth 是原 `agent_pending_actions` 与 checkpoint；根 Agent projection 只是授权视图。
- 重复、过期或已完成 decision 应返回 durable settlement，不会启动第二个 continuation。
- Approval 在 shutdown 快照后恢复的竞态由 Dispatcher 在 grace window 内重复扫描；未收口事实留给下次启动恢复。
- 凭据 backend 不可用导致 process-only MCP approval 时，重启后无法恢复原始参数；应让原 action 明确终结，再由用户重新发起。

## 5. 通知、observer 与 UI 恢复

以下 notification 都只是 invalidation：

- `agent.collaboration.event`
- `agent.collaboration.observerEvent`
- `agent.collaboration.resync`

出现缺卡、重复卡、顺序跳跃或 Core Server 重启时：

1. 核对当前 `rootConversationId`，立即丢弃其他根 Agent 的 notification。
2. 从 Electron Host API 重新获取 tree snapshot 和保守 replay cursor。
3. 使用 `agent.collaboration.listEvents(afterSequence)` 按根 Agent 本地 sequence 补洞。
4. 遇到 gap/乱序或 `resync` 时完整 rehydrate；不要用 occurredAt 排序覆盖 sequence。
5. 子 Agent observer 再加载 authoritative Conversation snapshot；live overlay 只用于低延迟，不替代持久消息。

页面刷新或进程内 `Notify` 丢失不应造成事实丢失。如果重新 hydrate 仍无法收敛，应优先视为数据库/协议 bug并保存 fixture，而不是清空 Renderer store 后宣称恢复成功。

## 6. Scheduled Automation 恢复

Scheduled Automation 只在应用和 Core Server 运行时调度；SQLite 中的 task、Run、Agent Turn identity、Trace、pending Approval、attention 与 notification outbox 才是恢复真相。`automation.event`、`automation.resync` 和 `notification_requested` 都只是唤醒/失效信号。详细状态机见 [Scheduled Automation](../subsystems/scheduled-automations.md)。

### 调度与 admission lease

Scheduler 每 30 秒 fallback scan，并在 create/update/enable/runNow、worker 完成等事件上提前唤醒。每轮最多读取/claim 3 条，Automation 专用并发上限为 2；容量或目标 Conversation 忙时，未 admission 的 Run 以 5 秒 backoff 返回 durable queue。

| 持久状态/事实                                                    | 恢复动作                                                                                |
| ---------------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| active、health `ok` 且 occurrence 在离线期间已 due               | 启动后合并为一个 trigger kind `recovery` 的 Run，并把 `nextRunAt` 推进到当前时间之后    |
| `queued`，没有 Agent Run/Conversation/message identity           | 等待 Scheduler claim；不要重复 `runNow`                                                 |
| `starting`（存储层 `admitting`），只有 admission token           | 进程内可在 60 秒 lease 到期后重 claim；Core Server 启动时确认旧进程已退出后会立即重排队 |
| `running`，已有完整 Agent Run/Conversation/message/Trace binding | 恢复 observer，继续观察原 Turn；绝不创建第二个根 Agent Turn                             |
| `waiting_for_approval` 且原 pending action 仍存在                | 恢复同一 observer、attention 与 Approval projection，等待对原 action 的批准或拒绝       |
| Run 仍非终态，但绑定 Trace 已终态                                | 从 Trace 做 durable settlement/补偿，不重跑 Provider 或 Tool                            |
| task health `blocked`                                            | 保持 `nextRunAt=null`，停止未来 claim；由用户修复配置，不自动降级权限或改绑资源         |

离线期间的 missed occurrence 不逐条 backfill；一次 recovery Run 是当前合并策略。`scheduledFor` 与 task/config revision 的唯一约束、短 `BEGIN IMMEDIATE` claim 以及根 Agent Turn 的原子 admission 共同防重复。不要把 60 秒 admission lease 理解为已绑定 Turn 的执行超时；一旦 durable identity 已建立，恢复依据是 Trace，而不是重新获取 lease。

### Approval 恢复

- 默认权限的 command/patch 可能让 Run 进入 `waiting_for_approval`；这是持久等待，不是失败。
- `agent_pending_actions`、Run binding 和 Conversation Trace 是 authority。Automation attention 与原生通知只负责把用户带回该 action。
- 重启后 approval/rejection 必须继续原 `agentRunId + actionId`；settlement 会 suppress 旧的 `approval_required` outbox，并继续同一 Turn，不新增用户 message。
- 若 UI 没有显示 Approval，先按 Run 的 `conversationId`、`assistantMessageId` 重新打开 authoritative Conversation，再刷新 pending action；不要再次运行任务或直接改 Run 为 `running`。
- Full 或 Custom 权限模式被设置关闭时，admission 会 fail closed，把未 admission Run 结算为 failed 并将 task 标为 `permission_disabled`；系统不会静默回退到 Default。

### notification outbox 恢复

Automation 原生通知由 Electron Main 消费 Core Server 的 durable outbox，与 Scheduler admission lease 完全分离：

1. Main 启动时立即 drain，此后每 30 秒 polling；Automation event/resync 只降低延迟。
2. 每批最多 10 条，delivery claim lease 为 60 秒。Main 在显示前向 Core Server 做最终语义校验；任务删除、blocked 已修复、Approval 已结算或 Run revision 已变化时会 suppress stale delivery。
3. 只有 Electron 发出 `show` 后才 ACK delivered。创建/显示/校验失败会 release 并设置 60 秒 retry；release 本身失败时，lease expiry 是后备恢复路径。
4. 同一 Main 进程会记住“已 show、未 ACK”，被重 claim 时只重试 ACK，不重复显示；如果进程恰在 show 与 ACK 之间崩溃，重启后仍可能重复一次。
5. 平台不支持原生通知时 Main 不 claim；outbox 与 attention 保留。当前没有 Renderer toast fallback，通知缺失不代表 Run 丢失。

恢复时先正常重启同一数据根并从 Scheduled 页面重新 list/get/history/attention。`blocked` 应按 code 修复 model/project/Conversation/schedule/permission 后保存新 revision；`queued`/`starting` 应检查 Core Server、容量和 lease，不手工改时间；`waiting_for_approval` 应回到精确 Conversation 处理 action。通知异常只处理 outbox/系统权限，不能改变 Run 结果。

## 7. MCP stdio 恢复

1. 从 MCP management status 获取 typed error，确认是 config、authorization、negotiation、catalog、timeout、transport close 还是 unknown outcome。
2. executable/cwd/code identity 改变时 authorization 应变 stale；通过原生预览重新授权，不复用旧 digest。
3. MCP Server crash 后 Manager 可以重连/重启连接，但不会自动重放可能发出的 Tool call。
4. stop/delete/shutdown 后在 Unix 检查 repository-owned fixture PID 是否消失；Windows 当前不能假设完整进程树已回收。
5. Catalog incomplete 时修复 cursor/schema/limit 问题并显式 refresh；不要调用部分 Catalog 中的 Tool。
6. 不把 stderr、Tool args/result 或凭据复制到普通诊断日志。只保留 safe error、server ID、revision 和 certainty。

## 8. Managed Playwright 恢复

- bridge request 的 dispatch phase 只能单调前进。已 `possibly_dispatched` 但没有完成响应的 Tool 必须按 unknown outcome 处理。
- Core Server shutdown 期间仍要让 Main 回传 `mcp.builtinPlaywright.dispatchPhase`/`mcp.builtinPlaywright.complete`；不要先切断 stdin 或添加外部 completion hook。
- Browser target/surface 崩溃由 Main broker 和 generation recovery 处理；不得复用用户浏览器 profile、remote debugging 或 DevTools 注入绕过生产信任边界。
- idle runtime 可在 10 分钟后关闭；下一次允许调用再按管理生命周期重建。
- packaged startup gate 不提供真实 Agent 驱动能力，不能用它排除业务链路故障。

## 9. 强制终止与关停超时

正常 `core.shutdown` 的顺序是：停止/取消 Browser risk → Managed Playwright runtime settlement（期间只继续接收反向 bridge 收口请求）→ MCP management stop admission → expiry reconciler → 各 dispatcher、图片执行、Multi-Agent Dispatcher、active Run、MCP tasks/Manager 有界并行收口 → 返回 shutdown response → drain outbound。

当前 Main watchdog 为 6 秒；Multi-Agent Dispatcher 的单阶段 `shutdown_grace` 为 5 秒，且算法最多经历“等待自行完成”和“取消后再扫描”两个阶段，理论最坏接近 10 秒。若 Main 先到 6 秒并强制终止 Core Server，不得把它记录为优雅关停；应保留 SQLite，并按 Wake/trace/checkpoint 分类恢复。

若 Main 的 shutdown watchdog 或子系统 grace 到期：

- 已接受但未完成的异步任务由 owner abort/join 或保留 durable recovery fact；
- 不应伪造成功终态来释放数据库 owner；
- 下一次启动按上述 reconciliation 分类；
- 连续复现应保存具体 subsystem shutdown report，并添加确定性测试，不能无限扩大 watchdog 掩盖泄漏。

## 10. 代码真源

- schema/reset：`crates/core/src/storage/migrations.rs`、`crates/core-server/src/bin/storage-reset-dev.rs`、`scripts/reset-dev-storage*.mjs`
- 数据库锁：`crates/core/src/storage/database_instance_lock.rs` 及 bootstrap 使用点
- Dispatcher/recovery：`crates/core-server/src/application/agent_dispatcher.rs`、`crates/core/src/storage/agent_graph_repository.rs`
- wait：`crates/core-server/src/application/agent_wait.rs`
- Approval/startup reconciliation：`crates/core-server/src/application/agent`、`transport/bootstrap.rs`
- MCP lifecycle：`crates/mcp-client/src/manager`、`transports/stdio.rs`
- managed bridge：`crates/core-server/src/application/mcp/managed_playwright_bridge.rs`、`src/main/core/managedPlaywrightBridge*`
- Renderer replay：`src/renderer/src/features/agentCollaboration`
- Scheduled Automation：`crates/core-server/src/application/automation`、`crates/core/src/storage/automation_repository.rs`
- Automation Main IPC：`src/main/ipc/automationIpc.ts`
- 通用系统通知：`src/main/ipc/notificationIpc.ts`、`src/main/notifications/systemNotificationCoordinator.ts`

## 11. 测试

```bash
pnpm test:storage-reset-dev
cargo test -p mycopilot-core --lib storage::migrations::tests
cargo test -p mycopilot-core-server application::agent_dispatcher
cargo test -p mycopilot-core-server application::agent_wait
pnpm test:multi-agent-release -- --smoke-only
cargo test -p mycopilot-mcp-client --test stdio_integration
pnpm exec vitest run --project managed-playwright-e2e
cargo test -p mycopilot-core automation_repository
cargo test -p mycopilot-core-server application::automation
pnpm test:automation-core-e2e
```

reset 变更必须覆盖 dry-run 只读、锁拒绝、备份不变、失败保留原库、配置保留矩阵、MCP identity、`quick_check`、foreign keys 和临时文件清理。恢复变更必须覆盖 lease deadline、重启、Approval race、unknown outcome 和通知 gap。Scheduled Automation 还应覆盖 missed-occurrence 合并、原子 admission、绑定 Trace 恢复、permission revocation、outbox claim/validate/show/ACK/release 与 stale suppression。

## 12. 当前限制

- 仅提供开发 reset，不提供生产原地迁移或通用 backup restore 工具。
- 备份没有自动 retention、加密或异地复制；其中可能含明文本机模型/搜索凭据。
- `outcome_unknown` 没有通用自动补偿，需要核验外部状态后显式发起新任务。
- Windows MCP stdio 没有 Job Object 进程树隔离保证。
- 通知正确性依赖 SQLite replay/polling，当前没有外部运维 dashboard 或 alert。
- Scheduled Automation 没有操作系统后台 Scheduler；应用退出时不执行，离线 missed occurrence 只合并为一个 recovery Run。
- 原生通知不受支持时没有 Renderer fallback；show 与 ACK 之间崩溃仍有一次重复显示窗口。
- Scheduled Automation 没有真实定时器、Provider/Approval、OS 通知点击、休眠唤醒或 packaged Electron 自动化 E2E。
- 没有 CI 自动运行恢复演练，也没有统一命令验证真实用户数据根；所有测试必须继续使用临时 fixture。
- Packaged Agent→Managed Playwright 的离线 E2E 尚未建立。

## 13. 变更检查表

- [ ] 是否先保护并记录原数据库/backup，且未手工改 schema 或状态？
- [ ] 新 schema 是否更新 version、fingerprint、fresh/legacy/tamper/reset tests？
- [ ] reset preservation allowlist 是否有明确安全理由和精确计数验证？
- [ ] 新长期任务是否有 durable identity、owner、lease、checkpoint 和 startup reconciliation？
- [ ] Scheduled Automation 是否区分 admission lease、已绑定 Trace 与 notification delivery lease，并证明 restart 不重复 Turn？
- [ ] Approval/outbox 恢复是否保持 exact identity、最终语义校验、ACK-after-show 与 stale suppression？
- [ ] 新外部副作用是否在无法证明安全时进入 `outcome_unknown`？
- [ ] 通知丢失后是否仍能从 authoritative snapshot/event log 收敛？
- [ ] Approval 与子 Agent observer 是否仍经根 Agent authority，不新增旁路？
- [ ] shutdown 是否停止 admission、有界收口并保留未完成事实供重启恢复？
- [ ] 是否补充故障注入、重启、锁、跨进程与目标平台测试？
- [ ] 是否更新本 Runbook、发布门禁和相应子系统文档？
