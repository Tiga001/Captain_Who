---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-09-16
---

# Scheduled Automation 子系统

本文定义 Captain Who 的 Automation（定时任务）在配置、调度、Agent 执行、持久化、实时更新和原生通知
上的当前契约。产品入口名为 **Scheduled**；它与
[Managed Playwright 浏览器自动化](browser-automation.md)是两个独立子系统。

## 1. 范围与边界

一条 Automation 是用户创建的持久任务配置；一次 Automation Run 是该配置在计划时间、手动触发或
重启恢复时生成的执行实例。权威状态在 Rust Core 管理的 SQLite 中，Renderer 只保存可丢弃的缓存和
编辑态。

```text
Scheduled UI
  -> Automation Host API / trusted IPC
  -> Main -> JSON-RPC
  -> Core Server AutomationService
       ├─ schedule normalization / target validation / permission resolution
       ├─ SQLite task, Run, event and Automation notification producer ledger
       └─ AutomationScheduler -> HumanRoot Turn -> normal Agent Runtime
                                      └─ automation_report

SQLite event -> Core Server notification -> Main -> Renderer invalidation/refetch
Automation producer ledger -> shared notification event/batch -> Main native notification
  -> trusted open request -> Scheduled/chat
```

职责划分：

- Renderer 负责列表、筛选、表单、详情 Drawer、Run 历史和 attention 展示，不计算调度权威结果。
- Main/Preload 负责受信 IPC、Core Server 生命周期、共享原生通知投递和点击后的窗口导航，不运行调度器。
- Core Server 负责 CRUD、CAS、目标与权限校验、调度 admission、恢复、Run 结算，以及 Automation/Notification 两类事件投影。
- Rust Core 负责 canonical schema、repository 事务、Agent Runtime 和 Automation 专用 Tool。

## 2. 任务配置

### 目标

Automation 支持两种 destination：

| destination     | 当前行为                                                                                          |
| --------------- | ------------------------------------------------------------------------------------------------- |
| `new_chat`      | 每次 Run 创建新的根 Conversation；配置冻结项目绑定、模型和模型推理配置投影                        |
| `existing_chat` | 在一个未归档、可写的活跃根 Conversation 中追加 HumanRoot Turn，并继承该 Conversation 的项目和模型 |

现有 Conversation 不能指向子 Agent Conversation 或非活跃 Agent 节点。创建、更新、`runNow` 和实际
admission 都会重新核对目标；项目/模型/Conversation 被删除、归档、禁用或路径失效时，任务进入
`blocked`，`nextRunAt` 被清空并产生 attention，而不是静默改绑。

### 调度

结构化 schedule 支持：

- 固定间隔：分钟、小时或天；
- 每日、工作日、每周指定星期；
- 自定义每 N 小时、日、周、月或年，并选择分钟、时间、星期、日期和月份。

Core Server 会规范化选择顺序、IANA timezone、anchor 和稳定 RRULE；RRULE 是派生的传输/存储投影，
Renderer 不提供自由文本编辑器，也不得从 `scheduleSummary` 反解析规则。固定间隔按 UTC 毫秒推进；
calendar schedule 按保存的 IANA timezone 计算。夏令时 spring-forward 中不存在的本地时间会跳过，
fall-back 的重复本地时间只采用较早的绝对时刻，避免同一 occurrence 执行两次。

当前 Scheduled UI 没有独立时区选择器。查看未保存任务时会保留原时区，但保存创建/编辑结果时会以
当前计算机的 IANA timezone 重建 schedule；跨时区编辑可能改变之后的触发时刻。

### 状态、并发和幂等

- 任务状态为 `active` 或 `paused`，健康状态另为 `ok` 或 `blocked`。暂停只清除未来调度，不取消已在
  执行的 Run；暂停任务仍可手动 `runNow`。
- 创建和 `runNow` 使用 `requestId` 幂等；更新、启停和删除使用 `expectedRevision` 做 CAS。
- 每个任务最多有一个非终态 Run；计划 occurrence 和手动 request 分别有 SQLite 唯一约束。
- 删除是 tombstone：停止未来调度、取消尚未 admission 的 Run、请求取消已运行/待审批 Run，并抑制
  尚未投递的通知。当前 UI 不提供恢复已删除任务的入口。
- 删除项目、Conversation 或模型时，先在同一事务内处理仍引用该资源的 tombstone：将健康状态改为
  `blocked`（保留已有阻塞原因），补齐缺失的历史名称/ID 快照，再由外键清空实际引用。此维护不改变
  删除标记、任务 revision、时间戳、调度、attention 或 Run 历史，不生成新的事件或通知；普通删除、
  关闭 trigger 的 Agent 树删除和新 Conversation 准备失败回滚使用相同处理。父资源删除失败时全部回滚，
  已有已删除任务也适用，无需清理历史或修改数据库 schema。

Automation 领域错误使用 JSON-RPC code `-32045` 和 closed typed data；客户端按 `not_found`、
`revision_conflict`、`validation`、`run_already_active`、`target_invalid`、`permission_disabled`、
`schedule_invalid` 或 `internal` 分支，不能解析英文 message 决定行为。

## 3. Run 与 Agent 执行

对外 Run 状态机为：

```text
queued -> starting -> running -> completed | failed | cancelled
                         \-> waiting_for_approval -> running | failed | cancelled
```

SQLite 内部还使用 `admitting`，协议层将它投影为 `starting`；客户端不得依赖内部状态名。触发类型为
`scheduled`、`manual` 或 `recovery`。

Scheduler 默认每 30 秒扫描，一批最多处理 3 个到期任务，并限制最多 2 个 Automation Run 并发。
Automation 同时共享进程级 Agent Turn 并发 gate，因此“2”只是该子系统上限，不是额外容量保证。
admission lease 为 60 秒，临时延后使用 5 秒 backoff；这些值的真源在
[`scheduler.rs`](../../crates/core-server/src/application/automation/scheduler.rs)。
Scheduler 只在应用与 Core Server 运行时工作；退出期间不会由操作系统后台唤醒，错过的 occurrence 在
下次启动按 recovery 规则合并处理。

每个 Run 在入队时冻结配置 snapshot；后续编辑不会改写已排队或已运行实例。成功 admission 会启动正常
HumanRoot Turn，并复用 Provider、上下文、Skill、MCP、Tool、Approval、Trace、取消和 Usage 链路。
Automation 元数据通过 Host 拥有的执行上下文注入，不作为伪造的用户 Prompt 前缀。

Automation Run 额外暴露只读、无需审批的 `automation_report` Tool。它最多成功调用一次，只能为当前 Run 写入一个结构化
结果：`no_change`、`important_update` 或 `completed`，摘要上限为 2,048 UTF-8 bytes；它不能修改任务
配置。模型未调用该 Tool 时，对外报告为 `unknown`。普通交互 Turn 不暴露该 Tool。终态
`resultPreview` 最多保留 8,192 UTF-8 bytes，错误 message 最多 4,096 UTF-8 bytes；这些有界投影不是
Exact Archive。

## 4. 权限与 Approval

任务表单提交 `default`、`full` 或 `custom` 模式及 permission mode v2；Core Server 根据保存当时的 UI
preferences 解析成完整 `AgentPermissions` 并冻结到任务/Run snapshot：

| 模式      | 冻结权限                                                                                 |
| --------- | ---------------------------------------------------------------------------------------- |
| `default` | workspace read/write；命令、patch 与内置能力执行需要 Approval；命令安全为 `guarded`      |
| `full`    | all read/write；命令、patch 与内置能力执行自动批准；命令安全为 `full_access`             |
| `custom`  | 使用当时自定义的 read/write/command/patch/builtinExecution，但命令安全强制保持 `guarded` |

`full`/`custom` 只有在当前 UI preferences 允许时才能保存。每个未来 Run admission 还会把当前开关作为
撤销上限复核：关闭某模式会阻止使用该模式的新 Run，但重新开启或修改自定义设置不会暗中放宽已有
snapshot；要改变冻结权限必须编辑任务。

后台 Run 仍走普通 Approval 状态机。需要用户处理时，Run 进入 `waiting_for_approval`、产生 attention，
并可发出原生通知；应用/进程重启后从持久 pending action 与 Trace 恢复，不默认批准。

Approval ticket 不因 MCP、Browser risk、Skill 安装等短生命周期执行材料过期而自动消失；它会保持待决定。用户稍后批准但材料已不可用时，正常 Run/Automation continuation 收到 definitely-not-dispatched 的 failed Tool Result，而不是伪装执行成功或重新 dispatch。

## 5. 实时更新与前端状态

Renderer 可调用 list/get/create/update/setEnabled/runNow/delete、runs.list、attention.summary 和
attention.acknowledge。`automation.event` 只表示缓存可能失效：Renderer 按 `sequence` 丢弃重复/倒序
事件，发现 gap 或收到 `automation.resync` 时全量重读；它不能从事件 payload 自行重建权威任务。
Core Server notifier 默认每 100 ms 从冻结 cursor 后读取最多 256 个持久事件；这些参数只影响延迟与
批量，不改变 SQLite sequence 的正确性。

Core Server 启动时发送 `core_started` resync。Main 会缓存 resync，Preload/Renderer 以
`resyncReady` 完成监听器就绪握手后再重放，避免窗口启动竞态。Renderer 的进程内缓存按 revision
拒绝旧响应覆盖新任务，并以 tombstone 防止已删除任务被迟到响应复活；同一任务同时只允许一个前端
mutation。

Scheduled 页面提供 All/Active/Paused、搜索、分页、attention 计数、创建/编辑、启停、立即执行、删除、
Run 历史和打开精确 Conversation/message。页面覆盖聊天和右侧栏时，被覆盖区域保持挂载但设为
`inert`/`aria-hidden`；详情 Drawer 的首选宽度只在当前应用会话内保留。

## 6. Attention 与原生通知

Attention 是需要用户查看的持久投影，当前包括待 Approval、Run 失败、重要更新和配置 blocked。确认
attention 只更新已读状态，不改变 Run 或任务配置。

Automation 通知 policy 按 destination 收窄：

| destination     | 可选 policy                              | 终态投递语义                                                                           |
| --------------- | ---------------------------------------- | -------------------------------------------------------------------------------------- |
| `new_chat`      | `all_runs`、`unsuccessful_only`          | 每次终态，或仅失败/取消                                                                |
| `existing_chat` | `important_updates`、`unsuccessful_only` | 失败/取消，以及报告为 `important_update`、`completed` 或 `unknown`；`no_change` 不通知 |

Approval 和配置 blocked 可产生对应通知。Automation producer 在业务事务内直接写入不可变的 shared
`notification_events`；同一通用通知管线生成并维护 `notification_batches`。前者是 Notification Center
的事件事实，后者是 native delivery 的权威事实，不再经过 Automation 专用 outbox 或兼容投影层。

shared pipeline 同时接收 HumanRoot 与 Automation：approval/configuration-blocked 的收集窗口为 500 ms，
failed 为 1 秒，其他为 2 秒；一个 batch 最长收集 5 秒、最多 100 项，并可在 60 秒 replacement window
内升级。Main 每次通过 `notifications.claim` 领取最多 10 个 batch，lease 为 60 秒，默认每 30 秒扫描；
`notification.event`、`notification.resync` 和 Automation invalidation 只用于降延迟/重读。Main 显示前
再次 `notifications.validate`，收到 Electron `show` 后才 acknowledge；展示失败或 15 秒未确认则 release，
最多尝试 5 次且单次 retry delay 不超过 60 秒。Renderer 不能调用 claim/validate/acknowledge/release/
suppress 这些 Host-only delivery RPC。

通知 event 的 seen、resolved、superseded 是持久 projection，不改写原始事实。协议虽定义
`notifications.list/summary`，当前只由 Main 为点击快照等 Host 流程使用，未暴露为 Renderer Host API；
Renderer 只使用 `markSeen`、settings、event/resync 和点击导航，当前没有应用内通知中心。点击通知时，Main
只发送经过严格解析的 `NotificationOpenRequest`：有 Conversation 时打开精确消息，
否则打开 Scheduled 任务/Run。合并通知选择当次可见快照中优先级最高、同级最新的可导航成员；窗口尚未
ready 时以 FIFO 暂存最多 32 个请求，并在 `openRequestedReady` 后恢复、显示和聚焦。

## 7. 持久化、恢复与删除联动

SQLite canonical schema 当前为 **v48**；Automation DTO 与 Automation 表记录的 `schemaVersion` 各为 **v1**，permission mode v2，shared Notification contract 为 v1；这些版本域不能混用。Automation 专属三组数据为：

- `automations`：配置、schedule、目标/权限 snapshot、revision、health、attention 和 tombstone；
- `automation_runs`：不可变配置 snapshot、admission lease、Agent/Conversation 绑定、终态和结果投影；
- `automation_events`：单调 sequence 的失效/重同步日志。

共享通知另使用 `notification_settings`、`notification_events`、`notification_batches`、
`notification_batch_items`、`notification_change_events`。event 是不可变通知事实及 list/summary 投影输入，
batch 是 native delivery authority；Main 原生投递只走当前 `notifications.*` 方法。

启动时 Core Server 回收全部上一进程遗留的 `admitting`（不等待旧 lease 过期）、把离线期间到期的 occurrence 标为 `recovery`、恢复
`running`/`waiting_for_approval` Run 的 Trace observer，再启动 Scheduler。持久 Trace 是 Agent Turn
终态真源；进程内 worker、timer、event notification 和 Renderer cache 都不是恢复依据。

删除/归档 Conversation、删除项目、删除或禁用模型时，schema trigger 与 repository 删除路径会先把
相关任务变成可修复的 blocked 状态，并终止尚未 admission 的 Run。已跨过原子 HumanRoot admission 的
Run 保持冻结 snapshot 并由正常取消/Trace 结算路径收口，避免在资源级联删除中留下悬空运行。

## 8. 安全与并发不变量

- Renderer 提交的目标名称、权限投影、reasoning、RRULE、Run 状态和通知内容都不是授权事实。
- 实际执行前必须重查目标有效性和当前 permission mode enablement，但不得重解析成更宽权限。
- Scheduler 不在等待 Provider、MCP、Approval 或全局并发 gate 时持有 SQLite transaction。
- 任务 revision、Run status revision、schedule occurrence、manual request、admission lease 和 shared batch claim 均
  由短事务/唯一约束保证；内存去重只优化延迟。
- 原生通知只含有界安全投影，不携带 Prompt、Tool 参数、原始结果、Token 或凭据。
- 删除、失效或 Approval 已结算后，已 claim 但尚未显示的通知必须再次验证并抑制。

## 9. 协议与代码真源

- TypeScript DTO/parser/method：
  [`packages/protocol/src/automations.ts`](../../packages/protocol/src/automations.ts)
- TypeScript/Rust 共享 fixture：
  [`automation-contract-v1.json`](../../packages/protocol/fixtures/automation-contract-v1.json)
- Rust DTO：[`automations.rs`](../../crates/protocol-rs/src/automations.rs)
- CRUD、目标校验与 DTO 投影：
  [`service.rs`](../../crates/core-server/src/application/automation/service.rs)
- schedule 与 Scheduler：
  [`schedule.rs`](../../crates/core-server/src/application/automation/schedule.rs)、
  [`scheduler.rs`](../../crates/core-server/src/application/automation/scheduler.rs)
- 权限：[`permissions.rs`](../../crates/core-server/src/application/automation/permissions.rs)
- Automation Turn：
  [`automation_turn.rs`](../../crates/core-server/src/application/agent/automation_turn.rs)
- repository 与 schema：
  [`automation_repository.rs`](../../crates/core/src/storage/automation_repository.rs)、
  [`canonical_schema.sql`](../../crates/core/src/storage/canonical_schema.sql)
- Automation 专用 Tool：
  [`automation_report.rs`](../../crates/core/src/tools/automation_report.rs)
- shared Notification DTO/fixture：
  [`notifications.ts`](../../packages/protocol/src/notifications.ts)、
  [`notification-contract-v1.json`](../../packages/protocol/fixtures/notification-contract-v1.json)
- shared Notification service/repository：
  [`notification.rs`](../../crates/core-server/src/application/notification.rs)、
  [`notification_rpc.rs`](../../crates/core-server/src/transport/notification_rpc.rs)、
  [`notification_repository.rs`](../../crates/core/src/storage/notification_repository.rs)
- Main/Preload：
  [`automationIpc.ts`](../../src/main/ipc/automationIpc.ts)、
  [`systemNotificationCoordinator.ts`](../../src/main/notifications/systemNotificationCoordinator.ts)、
  [`notificationIpc.ts`](../../src/main/ipc/notificationIpc.ts)、
  [`AutomationIpcBridge.ts`](../../src/preload/AutomationIpcBridge.ts)
- Renderer：
  [`features/automations`](../../src/renderer/src/features/automations)

## 10. 测试与验证

```bash
cargo test -p mycopilot-core
cargo test -p mycopilot-core-server
cargo test -p mycopilot-protocol-rs
pnpm exec vitest run --project unit packages/protocol/src/automations.test.ts src/main/core src/preload/AutomationIpcBridge.test.ts src/renderer/src/features/automations/__tests__
pnpm exec vitest run --project unit packages/protocol/src/notifications.test.ts src/main/notifications src/main/core/coreServer.notifications.test.ts
pnpm exec vitest run --project browser src/renderer/src/features/automations/__tests__
pnpm test:automation-core-e2e
```

Rust 测试覆盖 schedule/DST、CAS/唯一约束、lease、权限冻结、Scheduler/Approval/重启恢复、Automation
producer ledger、shared notification batching 和 Automation Turn；协议 fixture 同时由 TypeScript 与 Rust 消费。`automation-core-e2e` 启动真实的
Core Server，验证 Host API 的 CRUD、CAS、`runNow`、历史和 attention；但它是独立 project，当前
不在 `pnpm test:web`、`pnpm test` 或 `pnpm check` 中。

## 11. 当前限制

- UI 不能停止一个活动 Run；暂停只影响后续 schedule，删除会走取消请求。
- Automation attention 仍在 Scheduled 领域确认；notification event 的 seen 状态不会替代 attention acknowledge。系统不支持原生通知时仍没有 Renderer toast fallback 或应用内通知中心。
- Main 在“原生通知已显示、durable ACK 尚未完成”之间崩溃时，重启后可能重复显示一次。
- Renderer ready 前最多保留 32 个通知打开请求；极端连续点击溢出时丢弃最早请求。
- Scheduler 是单 Core Server 进程内轮询器，不是分布式 scheduler，也不承诺秒级准点。
- Run/history 没有独立 retention 或 purge 策略；删除任务使用 tombstone 并保留持久历史。
- 不支持 cron、一次性计划或结束日期；missed occurrences 会合并为一个 `recovery` Run，不逐次补跑。
- 配置 blocked 后不会因资源恢复或权限模式重新开启而自动解除，需要以有效配置执行一次 update。
- Automation 配置不接受附件、显式 Skill 选择、temperature/maxTokens 或独立 reasoning override；新
  Conversation 的 reasoning 只是模型配置投影，执行仍读取同一 model id 的当前模型配置，现有
  Conversation 也使用执行时的当前绑定。
- admission 与等待 Approval 没有统一次数/时长上限；shared native delivery 最多尝试 5 次，失败耗尽后转为 suppressed，但没有面向用户的独立 dead-letter 修复工作流。
- 真实 Core Server E2E 尚未覆盖定时到期、完整模型执行、Approval 往返、操作系统通知点击、进程重启和
  packaged 应用全链路。

## 12. 变更检查表

- [ ] 是否同时更新 TypeScript/Rust DTO、parser、schema v1 fixture 和契约测试？
- [ ] schedule 改动是否覆盖 IANA timezone、anchor、DST gap/fold 和 occurrence 幂等？
- [ ] mutation 是否保留 request idempotency 或 revision CAS？
- [ ] 权限是否由 Core Server 解析、冻结并在 admission 时仅作撤销复核？
- [ ] 新 Run 路径是否共享 Agent gate、Trace、Approval、取消、Usage 和恢复？
- [ ] 新事件是否只用于 invalidation，并覆盖 gap/resync/ready-handshake？
- [ ] 新 Automation 通知是否先写 producer ledger 并原子投影 shared event，再按 batch claim、validate、show、acknowledge/release？
- [ ] legacy `automation.notifications.*` 与 canonical `notifications.*` 是否没有被误当成两套 native delivery authority？
- [ ] 父资源删除、任务删除和关停是否不会留下悬空 Run 或失效通知？
- [ ] 是否运行 Rust、协议、Main/Preload、Renderer 和独立真实 Core Server E2E？
- [ ] 是否同步更新[测试策略](../development/testing.md)、[恢复 Runbook](../operations/recovery-runbook.md)
      与[威胁模型](../security/threat-model.md)？
