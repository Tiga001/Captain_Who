---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-09-16
---

# 恢复与故障处理 Runbook

本文用于开发和维护环境中的 Core Server、SQLite、Multi-Agent、FileChange、系统通知、MCP 与 Managed
Playwright 故障。优先原则是保护持久事实和外部副作用，不通过手工改库“修绿”。

## 1. 先确定故障域

| 现象                                                                 | 首要检查                                                                              | 不要先做                                      |
| -------------------------------------------------------------------- | ------------------------------------------------------------------------------------- | --------------------------------------------- |
| Core Server 无法启动，含 `development_storage_schema_reset_required` | schema version/fingerprint、数据根、完整错误码                                        | 手工改 `PRAGMA user_version` 或删表           |
| 提示 exact DB 已被占用                                               | 是否有 Captain Who/Core Server 进程仍运行；实例锁路径                                 | 删除 lock file 后继续运行两个 Core Server     |
| 子 Agent 长时间 queued/claimed                                       | Core Server 是否重启、Dispatcher 是否运行、lease deadline、数据库可写                 | 重复 spawn 同一任务或直接把 Wake 改 completed |
| 子 Agent 显示 running，但进程已崩溃                                  | trace/checkpoint/pending action/lease                                                 | 盲重放可能已有外部副作用的 Turn               |
| 子 Agent 等待审批                                                    | 根 Agent Approval projection、原 pending action、expiry                               | 从子 Agent 页面直接写 decision                |
| UI 丢活动或串线                                                      | 根 Agent ID、event sequence、resync、重新 hydrate                                     | 从 notification 时间戳重建状态                |
| FileChange 卡在 applying/显示 unknown                                | transaction、pending action、audit/receipt、权威文件 digest                           | 重放 `apply_patch` 或删除 journal/草稿行      |
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

当前基线为 **schema v53 + exact catalog fingerprint + valid foreign keys**；exact v52 → v53 新增任务归属活动明细，保留历史且不回填旧活动；exact v51 → v52 保留全部协作数据、回执和自增序号，仅更新 Mailbox 正文约束；exact v50 仍须协作事件日志为空，先升 v51、v52 再升 v53。旧协作活动不做父会话位置兼容或回填；v50 库需先用旧应用清理相关聊天历史，再启动新应用。v49 及更早版本不自动升级；显式开发 reset 的配置恢复仅支持工具列出的 exact catalog，不能据此推断它支持 v50。真源：

```text
crates/core/src/storage/migrations.rs
crates/core/src/storage/canonical_schema.sql
```

不支持的旧版本、非空未版本化库、catalog 漂移、外键违规或 v50 仍有旧协作事件的库返回 `development_storage_schema_reset_required`，不自动 reset。v53 不支持直接交给旧应用打开。

手动压缩在重启后显示 interrupted 时，可直接继续聊天；旧 active head 保持有效。不得重放原付费请求来“恢复进度”。如果请求已到达厂商但尚未收到响应就崩溃，实际账单可能只有厂商可确认，本地不能编造 token 数量。

### 预检

完全退出 Captain Who 后运行：

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
2. 从工具明确支持的 exact catalog 提取 allowlisted configuration；支持版本、指纹和受限私有备份恢复以 `crates/core-server/src/bin/storage-reset-dev.rs` 为准。MCP 精确 identity/authorization 必须重新验证；未知配置结构拒绝重置，不能用默认值默默替换模型配置。
3. 在 staging 文件创建 fresh v52 canonical DB。
4. 通过当前 service 写路径恢复配置。
5. 重开生产 storage，核对记录数、`PRAGMA quick_check` 和 `foreign_key_check`。
6. 原子发布新数据库；失败时保留原数据库与恢复备份。

保留内容：

- model/provider/search settings 及其 opaque credential reference；secret 本身由现有 Credential Store 持有，不复制进 fresh SQLite；
- UI preferences、Agent prompt preferences；
- 通知设置；源 catalog 支持时保留人机交互 enabled/revision/updatedAt、全局协作开关及 revision、上下文模式（轻量/完整）；旧源没有上下文模式时按工具的显式恢复规则使用 Full；
- Browser 链接打开位置、下载设置和偏好（仅在源 schema 支持对应表时）；
- Skill enablement overrides；
- 全部 image-generation profile 及其 credential reference；credential secret backend 本身不搬动、不清空；
- MCP Registry、model namespace、仍有效的 launch authorization 与 enabled/trust 状态，以及内建 Browser Automation 的用户允许策略。

不恢复：Conversation、Project、message、draft、Usage、Approval、Continuation、Compaction、Fork、Agent
tree/Mailbox/Wake；Scheduled Automation task/Run/attention；notification event/batch；Browser history/download
record；人机交互问题、回应、投递、忽略事件收据与挂起；全局 Agent 模板与 Project 分配；FileChange transaction/chunk/operation/run grant/history。Conversation 与上述 runtime 记录只计数并丢弃，不执行迁移。附件、已安装
Skill、生成图片和 credential 目录不在 reset 事务中移动；没有数据库引用的附件可在后续正常启动时被孤儿清理。

### 备份处理

- reset 失败时，优先保留原库；backup 是恢复/取证副本，不应被 reset 检查过程修改。
- 不要直接把一个旧 schema backup 覆盖回运行路径并期待当前版本接受；只有 exact v47 可升级，其余旧库仍触发 reset-required。
- 如必须人工还原文件，先停止所有 Core Server、再次复制保存当前文件、在隔离位置验证 SQLite 完整性和 schema，再决定是否替换。仓库当前没有受支持的一键 backup restore 命令。
- 当前 v52 SQLite backup 不含当前模型/搜索 secret，只含 reference 与非秘密元数据；旧 schema backup 仍可能含明文 Token/Key，二者都不得上传到 issue、CI artifact 或公共对象存储。
- 只恢复 SQLite 不会恢复操作系统凭据。跨设备、跨账户或凭据 backend 丢失后，保留的 reference 会显示为不可用，需要用户替换或清除。
- 未签名 macOS 开发构建的私有凭据文件属于数据根；整根备份会包含这些 secret。删除 backup、reference 或凭据文件不等于对 SSD、系统快照或外部备份安全擦除；怀疑泄露时应撤销或轮换 Provider 凭据。

### 旧开发库的配置保留边界

受支持的旧 exact catalog 使用上述受管 reset 流程提取白名单配置并新建 v52；受限私有备份恢复仍绑定固定 fingerprint。未支持的旧结构若包含任何配置表，工具拒绝重置并保留原库。不要修改版本号骗过检查，也不要临时写聊天、运行或检查点迁移绕过开发期数据策略。

重建后验证 `PRAGMA user_version = 49`、catalog fingerprint、`quick_check`、`foreign_key_check`，再核对模型、搜索和图片凭据状态，以及 UI/Prompt、Skill、MCP、通知、Browser 和人机交互设置。无需真实付费请求来验证配置保留；历史备份继续按敏感材料保管。

## 4. Multi-Agent 自动恢复

SQLite 是 Wake、Turn、Mailbox、receipt、Approval 和 event 的恢复真相。正常情况下，只需重新启动同一数据根的 Core Server：bootstrap 会在接收请求前启动 Dispatcher，并立即/周期扫描 queued 与 recoverable Wake。

### Wake 分类

| 持久状态                                                 | 恢复动作                                                          |
| -------------------------------------------------------- | ----------------------------------------------------------------- |
| `queued`                                                 | 按 FIFO 等待 Dispatcher claim                                     |
| 过期 `claimed`，无 Run identity                          | 安全重新排队/重新 claim；确定的 admission 前失败可结算 failed     |
| `running`，已有 terminal trace                           | 观察 terminal fact，完成 result outbox/owner 释放，不重跑 Runtime |
| `waiting_for_approval` 且 pending action 存在            | 保持等待；根 Agent UI 从 durable projection 恢复                  |
| `running` 且有合法 checkpoint                            | 按原 identity/permissions/capability snapshot 恢复                |
| `running` 且无可证明安全的 checkpoint                    | 结算 `outcome_unknown`，不重放 Tool/Provider 副作用               |
| exact run 被 durable tree fence 覆盖，且可证明未有副作用 | 取消未终态 action，将 Wake/Trace 安全结算为 `interrupted`         |
| exact run 被 fence 覆盖，但外部副作用是否发生不确定      | 保留证据并结算 `outcome_unknown`；不得以“已停止”为由盲重放        |

Wake lease 为 60 秒，Dispatcher 每 20 秒续租、每 1 秒 durable fallback scan。新 Core Server 启动时旧 lease 尚未过期属于正常窗口；周期扫描会在 deadline 后处理，不需要再次重启。

### 树级停止与 `interrupt_agent` 排障

Composer 停止仍通过既有 `cancelRun(runId)` 进入 Host，没有新增 Renderer 协议。排障时先核对该 ID 是否就是
当时仍活跃的根 Run，再查询它建立的 durable tree fence 及冻结的 descendant run membership；不要按 Agent 节点或
当前 display status 推测停止范围。fence 只覆盖停止时已接纳的 exact runs，不应吞掉之后显式创建的 Turn。冻结成员
的 Spawn/Send/Followup 与 action dispatch 必须被持久 gate 拒绝；已完成或被 fence 覆盖的子 Run 可以留下可读
result，但 settlement 不得再排队父 Agent Wake。

显式 `interrupt_agent` 的 `interrupt_requested` 是“运行时取消已投递”的 receipt，不是 durable terminal receipt。
短暂看到 `running` 可以是取消后的结算窗口，应继续观察 exact Wake/Trace，而不是重复创建任务。若进程在投递后、
写入 `dispatched_at` 前崩溃，启动恢复会幂等重投该请求；已有 `dispatched_at` 时不重复调用运行时中断，而是继续观察
或补齐结算。

重启扫描遇到 tree-stopped Run 时，先尊重已有 terminal trace；仍为 `in_progress` 且能证明外部副作用尚未发生的
Run 安全结算为 `interrupted`。如果 Command、MCP、Provider 或其他外部动作可能已经 dispatch、但缺少权威结果，
必须按 `outcome_unknown` 处理，不能为了让 UI 退出 `running` 而伪造 `interrupted`。任何终态或停止覆盖下的子
result 都只作为 Mailbox/observer 事实保留，不得重新唤醒父 Run。

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

## 5. FileChange 恢复

模型可见的普通文本/代码写入统一由 `apply_patch` 产生 FileChange。Direct update/delete 绑定当前 Run 对目标
`read_file` 的精确 Observation；create 绑定 Host 私有的 missing observation 并以 no-clobber 发布。Staged
事务将内容私有保存在 SQLite，单 chunk 最多 1 MiB、总内容最多 4 MiB、草稿 TTL 为 7 天；模型 Trace 与普通
Renderer event 不持有文件正文，只保留 body-free operation/digest。

### 状态与恢复动作

| 权威状态/现象                               | 恢复动作                                                                                      |
| ------------------------------------------- | --------------------------------------------------------------------------------------------- |
| `drafting` / `ready`                        | 以同一 owner 调 `status`，按返回的 `nextIndex`/`draftRevision` 继续或 `abort`；不重放旧 chunk |
| `waiting_approval`                          | 回到原 pending action；批准/拒绝必须绑定同一 action、transaction、Run 与 frozen proposal      |
| `applying` 且有 terminal audit/receipt      | 从 audit/receipt 结算同一事务，不再次 publication                                             |
| `applied` / `already_applied`               | 视为完成；UI diff 从 durable audit/history 按需读取                                           |
| `conflict`                                  | 重新读取目标并创建新提案；旧 observation/事务不能继续使用                                     |
| `outcome_unknown`                           | 检查权威文件与预期 target digest；精确命中可 reconciliation，否则停止并由用户决定补偿         |
| delete journal 为 prepared/tombstone 中间态 | 按 journal 与 dir/leaf identity reconcile/finalize 或 verified rollback，不手工移动 tombstone |

FileChange Run grant 是一次持久 authority：它精确绑定 Run、Conversation/Project、scope、目录 identity、批准
action 与 permission/tool/provider revisions。新的 grant 会撤销同 Run 的旧 active grant；审批被拒绝、过期、
替换或 Run 终止时 grant 失效。若恢复后 grant/Observation/目录 identity 无法重新证明，必须 fail closed。

`outcome_unknown` 不是“可重试”。先用受管读取验证目标：精确等于 frozen target 时记录为已发生；仍等于 base 或
出现第三种状态时保留未知/冲突证据并停止。symlink、hard link、special file、父目录替换或 delete tombstone
identity 不一致时不得通过 shell/手工文件移动绕过保护。

## 6. 通知、observer 与 UI 恢复

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

## 7. Scheduled Automation 恢复

Scheduled Automation 只在应用和 Core Server 运行时调度；SQLite 中的 task、Run、Agent Turn identity、Trace、pending Approval、attention 与通用 notification event/batch 才是恢复真相。`automation.event`、`automation.resync` 和 `notification_requested` 都只是唤醒/失效信号。详细状态机见 [Scheduled Automation](../subsystems/scheduled-automations.md)。

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
- 重启后 approval/rejection 必须继续原 `agentRunId + actionId`；settlement 会 resolve/suppress 旧的 `approval_required` Notification event/batch，并继续同一 Turn，不新增用户 message。
- 若 UI 没有显示 Approval，先按 Run 的 `conversationId`、`assistantMessageId` 重新打开 authoritative Conversation，再刷新 pending action；不要再次运行任务或直接改 Run 为 `running`。
- Full 或 Custom 权限模式被设置关闭时，admission 会 fail closed，把未 admission Run 结算为 failed 并将 task 标为 `permission_disabled`；系统不会静默回退到 Default。

### 通用系统通知 batch 恢复

Core Server 现在为普通根任务（`human_root`）和 Scheduled Automation（`automation`）统一保存 notification
event/batch，Electron Main 只有原生展示职责。它与 Scheduler admission lease 完全分离：

1. Main 启动时立即 drain，此后每 30 秒 polling；notification event 只降低延迟。
2. 每次 claim 最多 10 个 batch，delivery lease 为 60 秒。Main 在显示前重新校验；任务已读/删除、blocked 已修复、Approval 已结算或 revision 变化时会 suppress stale delivery。
3. 全局 disabled 会由 Rust Core 直接 suppress；应用前台或平台不支持原生通知时，Main claim 后以 foreground/disabled disposition 终态 suppress。它们不会无限保留 pending。
4. 只有 Electron 发出 `show` 后才 ACK delivered。创建/显示/校验失败会 release 并进入最长 60 秒的有界 backoff，最多 5 次；lease expiry 是 release 失败时的后备恢复路径。
5. 同一 Main 进程会记住“已 show、未 ACK”，被重 claim 时只重试 ACK；如果进程恰在 show 与 ACK 之间崩溃，重启后仍可能重复一次。
6. Electron 39 无稳定 replacement ID，普通 batch 增量只 ACK、不重复弹 toast；升级到 attention priority 时可替换一次。当前没有 Renderer toast fallback，通知缺失不代表任务事实丢失。

恢复时先正常重启同一数据根，并从所属 Conversation 或 Scheduled 页面重新读取权威状态。`blocked` 应按 code
修复 model/project/Conversation/schedule/permission 后保存新 revision；`queued`/`starting` 应检查 Core Server、
容量和 lease，不手工改时间；`waiting_for_approval` 应回到精确 Conversation 处理 action。通知异常只处理
notification settings/batch/系统权限，不能改变 Run 结果。

## 8. MCP stdio 恢复

1. 从 MCP management status 获取 typed error，确认是 config、authorization、negotiation、catalog、timeout、transport close 还是 unknown outcome。
2. executable/cwd/code identity 改变时 authorization 应变 stale；通过原生预览重新授权，不复用旧 digest。
3. MCP Server crash 后 Manager 可以重连/重启连接，但不会自动重放可能发出的 Tool call。
4. stop/delete/shutdown 后在 Unix 检查 repository-owned fixture PID 是否消失；Windows 当前不能假设完整进程树已回收。
5. Catalog incomplete 时修复 cursor/schema/limit 问题并显式 refresh；不要调用部分 Catalog 中的 Tool。
6. 不把 stderr、Tool args/result 或凭据复制到普通诊断日志。只保留 safe error、server ID、revision 和 certainty。

## 9. Managed Playwright 恢复

- bridge request 的 dispatch phase 只能单调前进。已 `possibly_dispatched` 但没有完成响应的 Tool 必须按 unknown outcome 处理。
- Core Server shutdown 期间仍要让 Main 回传 `mcp.builtinPlaywright.dispatchPhase`/`mcp.builtinPlaywright.complete`；不要先切断 stdin 或添加外部 completion hook。
- Browser target/surface 崩溃由 Main broker 和 generation recovery 处理；不得复用用户浏览器 profile、remote debugging 或 DevTools 注入绕过生产信任边界。
- idle runtime 可在 10 分钟后关闭；下一次允许调用再按管理生命周期重建。
- packaged startup gate 不提供真实 Agent 驱动能力，不能用它排除业务链路故障。

## 10. 强制终止与关停超时

正常 `core.shutdown` 的顺序是：停止/取消 Browser risk → Managed Playwright runtime settlement（期间只继续接收反向 bridge 收口请求）→ MCP management stop admission → expiry reconciler → 各 dispatcher、图片执行、Multi-Agent Dispatcher、active Run、MCP tasks/Manager 有界并行收口 → 返回 shutdown response → drain outbound。

当前 Main watchdog 为 6 秒；Multi-Agent Dispatcher 的单阶段 `shutdown_grace` 为 5 秒，且算法最多经历“等待自行完成”和“取消后再扫描”两个阶段，理论最坏接近 10 秒。若 Main 先到 6 秒并强制终止 Core Server，不得把它记录为优雅关停；应保留 SQLite，并按 Wake/trace/checkpoint 分类恢复。

若 Main 的 shutdown watchdog 或子系统 grace 到期：

- 已接受但未完成的异步任务由 owner abort/join 或保留 durable recovery fact；
- 不应伪造成功终态来释放数据库 owner；
- 下一次启动按上述 reconciliation 分类；
- 连续复现应保存具体 subsystem shutdown report，并添加确定性测试，不能无限扩大 watchdog 掩盖泄漏。

## 11. 代码真源

- schema/reset：`crates/core/src/storage/migrations.rs`、`crates/core-server/src/bin/storage-reset-dev.rs`、`scripts/reset-dev-storage*.mjs`
- 数据库锁：`crates/core/src/storage/database_instance_lock.rs` 及 bootstrap 使用点
- Dispatcher/recovery：`crates/core-server/src/application/agent_dispatcher.rs`、`crates/core/src/storage/agent_graph_repository.rs`
- wait：`crates/core-server/src/application/agent_wait.rs`
- Approval/startup reconciliation：`crates/core-server/src/application/agent`、`transport/bootstrap.rs`
- FileChange：`crates/core/src/file_change`、`crates/core/src/tools/apply_patch.rs`、`crates/core/src/storage/file_change_repository.rs`、`crates/core/src/storage/file_change_run_grant_repository.rs`
- MCP lifecycle：`crates/mcp-client/src/manager`、`transports/stdio.rs`
- managed bridge：`crates/core-server/src/application/mcp/managed_playwright_bridge.rs`、`src/main/core/managedPlaywrightBridge*`
- Renderer replay：`src/renderer/src/features/agentCollaboration`
- Scheduled Automation：`crates/core-server/src/application/automation`、`crates/core/src/storage/automation_repository.rs`
- Automation Main IPC：`src/main/ipc/automationIpc.ts`
- 通用系统通知：`src/main/ipc/notificationIpc.ts`、`src/main/notifications/systemNotificationCoordinator.ts`

## 12. 测试

```bash
pnpm test:storage-reset-dev
cargo test -p mycopilot-core --lib storage::migrations::tests
cargo test -p mycopilot-core-server application::agent_dispatcher
cargo test -p mycopilot-core-server application::agent_wait
cargo test -p mycopilot-core file_change
cargo test -p mycopilot-core-server file_change
pnpm test:multi-agent-release -- --smoke-only
cargo test -p mycopilot-mcp-client --test stdio_integration
pnpm exec vitest run --project managed-playwright-e2e
cargo test -p mycopilot-core automation_repository
cargo test -p mycopilot-core-server application::automation
pnpm test:automation-core-e2e
```

reset 变更必须覆盖 dry-run 只读、锁拒绝、备份不变、失败保留原库、配置保留矩阵、MCP identity、`quick_check`、foreign keys 和临时文件清理。恢复变更必须覆盖 lease deadline、重启、Approval race、unknown outcome 和通知 gap。FileChange 还应覆盖 Observation/Run-grant identity、staged replay/CAS、publication 前后故障、delete journal 与 audit/Trace 原子性。Scheduled Automation 还应覆盖 missed-occurrence 合并、原子 admission、绑定 Trace 恢复、permission revocation、batch claim/validate/show/ACK/release 与 stale suppression。

## 13. 当前限制

- 仅提供开发 reset，不提供生产原地迁移或通用 backup restore 工具。
- 备份没有自动 retention、加密或异地复制；当前 v52 SQLite snapshot 不含当前模型/搜索 secret，但旧 schema backup 或未签名 macOS 开发环境的整根备份可能含明文凭据。
- `outcome_unknown` 没有通用自动补偿，需要核验外部状态后显式发起新任务。
- Windows MCP stdio 没有 Job Object 进程树隔离保证。
- 通知正确性依赖 SQLite replay/polling，当前没有外部运维 dashboard 或 alert；前台/disabled/不支持场景会终态 suppress，不能事后补发。
- Scheduled Automation 没有操作系统后台 Scheduler；应用退出时不执行，离线 missed occurrence 只合并为一个 recovery Run。
- 原生通知不受支持时没有 Renderer fallback；show 与 ACK 之间崩溃仍有一次重复显示窗口。
- Scheduled Automation 没有真实定时器、Provider/Approval、OS 通知点击、休眠唤醒或 packaged Electron 自动化 E2E。
- 没有 CI 自动运行恢复演练，也没有统一命令验证真实用户数据根；所有测试必须继续使用临时 fixture。
- Packaged Agent→Managed Playwright 的离线 E2E 尚未建立。
- FileChange 只支持普通 UTF-8 文本、单目标且最多 4 MiB；Office/Artifact/二进制输出仍走各自受管发布路径。

## 14. 变更检查表

- [ ] 是否先保护并记录原数据库/backup，且未手工改 schema 或状态？
- [ ] 新 schema 是否更新 version、fingerprint、fresh/legacy/tamper/reset tests？
- [ ] reset preservation allowlist 是否有明确安全理由和精确计数验证？
- [ ] 新长期任务是否有 durable identity、owner、lease、checkpoint 和 startup reconciliation？
- [ ] Scheduled Automation 是否区分 admission lease、已绑定 Trace 与 notification delivery lease，并证明 restart 不重复 Turn？
- [ ] Approval/notification batch 恢复是否保持 exact identity、最终语义校验、ACK-after-show 与 stale suppression？
- [ ] 新外部副作用是否在无法证明安全时进入 `outcome_unknown`？
- [ ] FileChange 是否仍绑定 read Observation、Run grant、目录 identity、audit/receipt，并覆盖 publication/delete crash window？
- [ ] 通知丢失后是否仍能从 authoritative snapshot/event log 收敛？
- [ ] Approval 与子 Agent observer 是否仍经根 Agent authority，不新增旁路？
- [ ] shutdown 是否停止 admission、有界收口并保留未完成事实供重启恢复？
- [ ] 是否补充故障注入、重启、锁、跨进程与目标平台测试？
- [ ] 是否更新本 Runbook、发布门禁和相应子系统文档？

## 15. 人机交互等待与回答投递

问题状态和投递状态必须分别检查。`submitted` 表示不可变答案已保存，不保证模型已消费；等待合法安全边界时不要重新开放问题或生成另一条用户消息。

- `waiting_for_user_input`：保持同逻辑 Run 占用。客户端从独立问题查询恢复入口；不能用审批 Approved、普通 User 或轮询模型替代原工具恢复。
- 同步 `waiting` 与已保存未领取回答可恢复；`claimed` 未执行可重新领取。`executing` 后只有原 ToolResult 在 trace 和模型上下文中的完整证明、可靠后继检查点或终态事实才允许结算；没有证明时保守失败，禁止盲重放工具或付费请求。
- 异步 `open`：自然完成后继续保留；通过原 requestId 整批提交才是新用户意图。`pending/bound` 的已提交回答由 Host 重新选择合法投递路径，Renderer 不 startRun。
- 审批、同步提问和压缩期间：没有 worker 也不等于聊天空闲。答案先落库，等待这些操作释放聊天后调度。
- Stop：取消停止范围内此前接纳且尚未应用的回应，迟到工作和重启不得重新启动。Stop 后用户主动提交此前未答的异步批次属于新的明确输入。
- 忽略：只结束当前异步批次，没有 User、guidance、Wake 或模型请求；不能为通知“已忽略”额外启动推理。
- 多窗口、重连：重新查询完整分页，Request/Delivery 各自按 revision 收敛。终态批次不能因迟到通知重新开放；输入草稿只在原窗口内存中保存，窗口重载后不恢复草稿。
- 答案展示：只读 `human_interaction_message_projections` 是冻结历史证明，不能作为提交或恢复权限。Fork 可以复制边界内的此证明，但不能复制 request/delivery/suspension。不要从普通 User JSON 或通用 `historical_snapshot` 身份手工生成证明。

若提交结果未知，前端保留原 payload 和 submissionId，重试必须使用同一身份；已结算批次不可编辑。已保存但恢复失败时保留权威回答和错误事实，先排查原 Provider 身份、凭据、检查点及停止围栏，再由用户明确继续。

验证入口为 `pnpm test:human-interaction-core-e2e` 和[人机交互子系统验收矩阵](../subsystems/human-interaction.md)。上述恢复仅适用于同版本 canonical schema；旧开发库按第 3 节 reset，不迁移旧聊天或检查点。
