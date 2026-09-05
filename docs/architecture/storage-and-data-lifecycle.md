---
status: current
audience: developers
owner: engineering
last_verified: 2026-09-05
---

# SQLite 存储与数据生命周期

本文说明 Rust Core 的本地 SQLite 权威存储、schema 发布策略、事务边界、文件型数据和启动恢复。本文不复制完整 DDL；`canonical_schema.sql` 及其 fingerprint 测试是唯一 schema 真源。Trace、上下文、FileChange 和 Scheduled Automation 的逻辑契约分别见[Conversation Trace 与 Exact Archive](./conversation-trace-and-archive.md)、[上下文管理](./context-management.md)、[FileChange 子系统](../subsystems/file-change.md)和[Scheduled Automation 子系统](../subsystems/scheduled-automations.md)。

## 职责边界

- `StorageService` 向 application 层提供领域操作；repository 负责单一表组的 SQL 与映射。
- Core Server 负责把一次业务结算组合成事务，不允许 Renderer 直接写数据库。
- SQLite 保存元数据、消息、Trace、审批、Run 恢复、设置和索引；大型附件、受管 Artifact、命令 spool 等内容保存在受管文件目录，SQLite 保存身份、hash、授权和生命周期。
- 模型 Token、Tavily Key 与图片生成 API Key 和普通配置分离。SQLite 只保存不透明 credential reference、状态与非秘密元数据；具备稳定签名身份的发行构建使用操作系统凭据存储，未签名 macOS 开发构建使用应用数据根内的私有文件 backend（目录 `0700`、文件 `0600`）。所有凭据都不得进入日志、Trace 或普通 Renderer 投影。

## 数据库定位与单实例

应用数据根目录由启动配置解析，开发/测试可使用 `MYCOPILOT_APP_DATA_ROOT`；独立存储场景可使用 `MYCOPILOT_STORAGE_DB`。生产代码应通过 bootstrap 返回的权威路径，不在各模块自行拼接默认目录。

打开数据库时启用 foreign keys、busy timeout 和 canonical schema 验证。一个进程通过 database instance lock 独占同一权威数据库；`StorageService` 当前以单个受 Mutex 保护的 `rusqlite::Connection` 串行化访问。不要绕过服务另开写连接，否则会破坏锁、事务和启动 reconciliation 假设。

## Schema 发布策略

截至本次核验，当前唯一受支持的 canonical schema 是 **v35**（SQLite `PRAGMA user_version = 35`）：

- `STORAGE_SCHEMA_VERSION = 35`；
- canonical schema fingerprint 为 `sha256:794df50e6e2db70432cb0a233befa5cda47ff7069e98dd3ae53d94fd87a45c4b`，由编译期常量和测试固定；
- 空数据库在一个原子流程中建立完整当前 schema；
- exact v34 catalog、fingerprint 与外键均通过检查时，在一个事务内追加手动压缩操作和独立用量表并升级到 v35；升级后再次校验完整 catalog 与外键，失败回滚，保留所有原始历史；
- 其他旧版、未知版、非空未版本化或结构被篡改的数据库返回 `development_storage_schema_reset_required`，不自动重置。

版本号和 fingerprint 可能变化，维护时必须读取 `crates/core/src/storage/migrations.rs`，不得从本文复制常量到运行逻辑。发布说明可以记录版本，但架构文档应强调策略而非长期维护一张迁移历史表。

开发库重置前应先关闭应用并备份数据根；优先使用受管 `storage:reset-dev` 流程。不要只删除 `storage.sqlite` 而遗留 attachments、artifacts、spool 或 lock 文件。

`storage:reset-dev` 与正常 v34 → v35 保数据升级是不同操作：reset 始终新建 v35，不恢复 Conversation、Project 或 Agent/runtime 历史。它只保留 exact current schema 的 allowlisted 配置；已有的受限恢复选项也可从绑定 exact v33 fingerprint 的私有备份读取 allowlisted 设置，并把凭据转换为 reference。未知历史库仍拒绝，不能把 reset 当作正常升级步骤。

## 领域数据地图

DDL 按领域大致分为：

| 领域                  | 代表数据                                                                                                  | 生命周期要点                                          |
| --------------------- | --------------------------------------------------------------------------------------------------------- | ----------------------------------------------------- |
| 配置与目录            | Provider/model、MCP Server registry/policy、image profile、项目、模板、偏好                               | revision/CAS；secret 与公开配置分离                   |
| 会话内容              | conversations、messages、attachments、drafts、guidance                                                    | 会话/项目归属；附件文件与行记录一致提交               |
| Agent Run             | usage、pending actions、action audit、Turn diff、world state                                              | Run/call/action identity 幂等；审批状态单向推进       |
| FileChange            | Staged transaction/chunk/operation、Run grant、terminal action audit                                      | Direct/Staged 共用 receipt；grant runtime-only        |
| Trace 与上下文        | Turn Trace/items、model-context、history blob/chunk、compaction summary/head/receipt、request observation | append-only 或 immutable 派生；rewrite/fork 显式事务  |
| Provider continuation | encrypted envelopes、Tool Call bindings、transition records                                               | 私有、加密、绑定 profile/revision；promotion/release  |
| Command Session       | session、output chunks、published outputs、read receipts、lifecycle                                       | 顺序 transcript；重启后保守结算/恢复                  |
| Skills                | enablement override、安装/来源相关持久快照                                                                | Skill 包内容寻址；安装发布 CAS 与 tombstone           |
| Artifacts/图像        | managed Artifacts/grants、generation execution/Artifact/config staging                                    | 内容寻址；授权与私有路径分开；启动清理                |
| Browser               | download settings/preferences/history、durable downloads                                                  | Host 捕获字节；Rust Core 保存身份、hash、scope 与历史 |
| 多 Agent              | nodes、mailbox、wake/interrupt、delivery receipt/replay、context snapshot、collaboration event            | 图和队列限制在事务内复核；cursor/receipt 幂等         |
| Scheduled Automation  | Task、Run、Event、Automation notification producer ledger                                                 | CAS、非重叠 Run、lease、Trace 恢复与 tombstone        |
| Shared Notification   | settings、immutable event、batch/item、change event                                                       | HumanRoot/Automation 共用；batch 是原生投递 authority |
| 搜索索引              | message/archive FTS 等                                                                                    | 可重建，不是权威内容                                  |

新增表前先确定领域 owner、父对象、删除策略、敏感级别、幂等键和恢复行为。不要把跨领域工作流塞进单个 repository。

## Scheduled Automation 表组

Automation 在 canonical schema v35 中使用当前通用通知表和自动化领域表，完整列、CHECK、索引和 trigger 仍以 DDL 为准：

| 表                  | 权威内容                                                                | 关键不变量                                                                                                        |
| ------------------- | ----------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| `automations`       | Task 配置、状态/健康、目标快照、权限/推理/schedule JSON、next/last、CAS | `active` / `paused` 与 `ok` / `blocked` 正交；revision 单调；paused/blocked/tombstone 时没有未来 `next_run_at`    |
| `automation_runs`   | 不可变配置快照、trigger、admission lease、Agent/消息绑定、报告与终态    | 同 Task 只允许一个非终态 Run；scheduled occurrence 和 manual request 各自唯一；内部 `admitting` 对外为 `starting` |
| `automation_events` | 全局有序的失效/refetch 事件                                             | AUTOINCREMENT sequence；Task/Run revision 与事件绑定；公开通知不携带内部 payload JSON                             |

Task 的权限、destination、schedule 和 reasoning 使用版本化 JSON；Run 在排队时保存最多 128 KiB 的 `config_snapshot_json`，worker 不得从更新后的 Task 重建 authority。数据库唯一索引负责 manual request 幂等、同一 scheduled occurrence 去重和每 Task 单非终态 Run，进程内集合或 Renderer 缓存不承担这些约束。

Automation 删除当前是 tombstone，不物理删除 Task/Run/Event；pending 原生通知被 suppress。Conversation/project/model 删除或归档/禁用通过 canonical trigger 将仍存在的 Task 变成可修复的 `blocked`；Agent-tree 删除事务会暂时禁用 trigger，因此必须调用 `invalidate_automations_before_trigger_disabled_*` 和 `terminalize_automation_runs_before_*` 在父消息/Trace 消失前投影等价效果。

HumanRoot 与 Automation 共用 `notification_settings`、`notification_events`、
`notification_batches`、`notification_batch_items`、`notification_change_events`。event 是不可变事实，
seen/resolved/superseded 是其 projection；batch 负责 collecting/pending/claimed/displayed/sealed/suppressed
原生投递生命周期。Automation producer 在同一业务事务中直接写入 generic event；不得重新引入独立的
Automation notification outbox、投递 RPC 或第二套 claim/acknowledge authority。

## 手动压缩与独立用量

`manual_context_compaction_operations` 保存稳定 request/operation identity、冻结模型与历史锚点、状态和时间。`manual_context_compaction_usage_records` 以 operation 为唯一计费 owner，使用请求开始时的价格，不覆盖 `agent_usage_records` 中上一条回复。已收到响应的真实用量先独立落盘，再原子提交摘要、receipt、active head 和操作终态。失败或取消也保留已经观察到的费用；未返回的 token 数据为未知。

用量汇总增加请求数、tokens 和估算费用，不增加聊天消息数。清理用量隐藏计费行但保留幂等凭证，迟到重放不会重新计费；删除聊天前汇入删除后的日汇总。latest fork 克隆摘要、continuation 与展示边界，不克隆费用。

## 事务原则

1. **外部调用不占写事务。** 模型请求、命令执行、网络下载和摘要生成在事务外完成；提交时重新验证 revision/归属。
2. **先持久授权，再开始副作用。** pending action 的批准与 audit/CAS 在事务中完成，事务提交后才执行 Core Server-owned 动作。
3. **终态一起提交。** assistant message、Trace/model-context 终态、Usage 和相关 projection 在同一业务边界写入，避免 UI 成功而历史未结算。
4. **Immutable + head。** Compaction summary、Archive blob、Artifact 内容等不可变对象与当前 head/grant 分离。
5. **幂等 identity。** `request_id`、`run_id`、`call_id`、`action_id`、execution fingerprint 等必须在数据库约束/CAS 下判定，不靠内存去重。
6. **文件采用 staging + fsync/原子发布。** 数据库记录和最终文件路径必须有明确提交顺序及启动清理策略。
7. **调用方事务。** 标注 `*_in_transaction` 的 repository 函数不自行 begin/commit；事务所有权属于组合业务操作。
8. **文件副作用有独立 journal。** FileChange 在 pending/audit 中冻结 exact binding，先持久 dispatch 边界，再以 bound I/O 发布并保存 receipt；Renderer Diff 或 Trace digest 不能替代该 journal。

Automation 还要求两个专用原子边界：

- scheduled enqueue 在一个 `BEGIN IMMEDIATE` 中写入冻结 Run snapshot、记录事件，并把 Task 的 `last_scheduled_at`/`next_run_at` 推进到严格未来；stale scheduler 不能覆盖较新的 Task revision；
- HumanRoot admission 在同一个调用方事务中写 Conversation、user/assistant message、空的 in-progress Trace、delivery binding，并把 `automation_runs.admitting` 绑定为 `running`。Full/Custom 权限的当前启用状态也在这个线性化点复核；stale token、取消或权限撤销不能留下孤立消息。

## 启动与崩溃恢复

Bootstrap 大致执行：解析数据根与锁、打开/校验 canonical schema v35、构造 repositories/services、加载凭据 backend、MCP Server/Provider/Skills/Artifact Runtime、随后运行领域 reconciliation。持久凭据 backend 不可用或 credential reference 无法解析时必须保留公开配置并报告 `unavailable`/配置错误，不能把缺失凭据当作空值覆盖；只有实际需要该连接的运行应被阻断。

恢复必须按“数据库已提交状态”判断，不按 Renderer 缓存判断。当前需要关注：

- orphaned `in_progress` Trace 和未终结 assistant Run；
- 未完成手动压缩标记为 interrupted，不自动重放模型请求；已观察用量保留，已提交摘要仍以事务事实为准；
- interrupted pending action、approval successor 和 action audit；
- Provider continuation prepared/promoted/released 状态；
- Command Session 进程已消失、transcript/receipt 尚未结算；
- image generation/admitted execution 的 succeeded、failed 或 indeterminate；
- Artifact staging、临时文件、过期 grant/安装 session；
- collaboration delivery、wake/interrupt 和子 Agent snapshot receipt。
- Automation 的 `admitting` lease、已绑定 `running`/`waiting_for_approval` Run、删除取消请求、Trace 终态和 durable pending action。
- FileChange 的 executing action audit、`applying` Staged transaction、Direct delete journal，以及 pending/active Run grant。
- shared Notification collecting/claimed/displayed batch、过期 claim 与 immutable event/change sequence；进程通知丢失后从 cursor 重放。

Automation 启动恢复区分 admission 前后：旧进程遗留的全部 `admitting` 立即回到 `queued`（已 tombstone Task 则取消），不等待旧 60 秒 lease；已经原子绑定的 `running`/`waiting_for_approval` 不重建消息，只从准确的 Conversation Trace 和 pending action 恢复 observer。多个离线 missed occurrence 合并为一个 `recovery` Run，而不是逐条补跑。进程内 scheduler wake 和 Agent event 只降延迟，不能代替这些行。

当外部副作用可能已发生但数据库没有成功确认时，恢复结果必须使用 `outcome_indeterminate`/`commit_indeterminate` 等保守状态，不能自动重放付费生成、写文件或命令。

## 敏感数据与文件生命周期

- API key、Provider credential、continuation 加密主密钥不进入日志、Trace 或普通配置行。
- Renderer 只接收 `missing`、`configured`、`unavailable` 等状态，以及用户本次新输入的替换值；Host 永不回传已有 secret 或不透明 credential reference。
- runtime 在开始请求前按冻结的连接 revision 解析所选模型需要的凭据；事务和 SQLite mutex 内不得执行 OS Keychain、Secret Service、Credential Manager 或开发凭据文件 I/O。
- Provider continuation payload 加密后存储，元数据仍必须最小化并绑定归属。
- Attachment、Artifact、Browser download、Skill package 和 command workspace 通过统一 locator/service 解析 URI/ID；数据库路径不是 Renderer/模型 API。未知 `scheme:` 或 `@namespace` fail closed；需要同名本地文件时用显式 `./...` 消歧。
- Artifact 使用内容 hash 标识与 conversation grant 分离；Browser download 以 `browser-download:<uuid>` 暴露且不含宿主路径。当前 Conversation 之外的访问必须由 Host 从 Project 或 `agent_nodes` 中的 exact `root_agent_id + root_conversation_id` 推导，模型/Renderer 自报 root 无效。
- 同一 Agent task tree 内，attachment library、managed Artifact 和 Agent browser download 可以父/子/兄弟双向复用，即使没有 Project；普通 Conversation 与其他根树继续隔离。Agent 完成或归档不自动撤销 durable child result 的树内可读性。
- Exact Archive 只保存安全文本投影；二进制和 data URL 被省略并记录诊断。
- action audit 可能有意晚于消息内容存活；删除策略必须由 service 显式实现，不能假设所有外键都 cascade。

## 备份、恢复与可观测性

当前开发策略以整套数据根备份/重置为主。复制在线 SQLite 文件并不等价于一致备份；应在应用关闭、锁释放后复制数据库及其配套文件目录，或使用受支持的 SQLite snapshot/backup 流程。

当前 v35 SQLite snapshot 只含模型/搜索 credential reference 与非秘密元数据，不含这些连接的当前 secret 字节；旧 schema 生成的历史备份仍可能含明文 Token/Key，必须继续按秘密材料保护。只恢复 `storage.sqlite` 不会恢复操作系统凭据，跨设备、跨系统账户、签名身份变化或凭据 backend 丢失后，界面可能显示凭据不可用，此时只能由用户替换或清除。未签名 macOS 开发构建的私有凭据文件位于数据根，因此“整根复制”仍会复制 secret，不能当作普通诊断包。

删除 SQLite reference、清除凭据或移除私有文件只表达应用层删除意图；文件系统、SSD、系统备份和操作系统凭据后端可能保留副本，产品不承诺安全擦除。怀疑泄露时应在 Provider 侧撤销或轮换凭据。

错误日志可包含 schema version、fingerprint、record identity、phase 和 recovery code，但不得打印消息正文、原始 Tool 参数、continuation 密文解密结果、密钥或私有绝对路径。

## 不变量

1. `canonical_schema.sql`、canonical schema v35 的 version 与 fingerprint 必须一致。
2. 非空非当前 schema fail closed，不自动执行未审计迁移。
3. 所有领域对象在 service SQL 边界校验 conversation/project/Run 归属。
4. 外部副作用与数据库提交之间的崩溃窗口必须有明确恢复状态。
5. FTS、Renderer JSON 和缓存均不是权威数据。
6. 内容寻址对象的 hash 针对发布后不可变字节；mutable head/grant 单独更新。
7. 删除、rewrite、fork 和 startup reconciliation 必须保持跨表/文件引用完整。
8. Automation 的 Task revision、Run status revision、admission token 和 Event sequence 只能在 repository 事务中推进；公开 `starting` 不得反向写成新的持久状态。
9. 已绑定 Automation Run 只能由匹配的 durable Trace 结算；admission 前失败不得伪造 Conversation/Trace 身份。
10. FileChange visible Staged history 与 terminal audit 可按 fork policy 重映射；pending/executing action 和 Run grant 不复制。
11. Notification event 是不可变事实，batch 是 native delivery authority；legacy Automation ledger 只能原子投影，不能单独驱动 Main 重复显示。
12. Agent-tree 私有资源 scope 只能从持久 `agent_nodes` 解析，不能接受调用参数提供 root identity。

## 代码真源

- Schema/version：`crates/core/src/storage/canonical_schema.sql`、`migrations.rs`
- 服务与连接：`crates/core/src/storage/service.rs`、`storage/mod.rs`
- 单实例：`crates/core/src/storage/database_instance_lock.rs`
- 快照：`crates/core/src/storage/database_snapshot.rs`
- Repositories：`crates/core/src/storage/*_repository.rs`
- 领域组合事务：`crates/core/src/storage/service/`
- Automation：`crates/core/src/storage/automation_repository.rs`、`storage/service/automations.rs`
- FileChange：`crates/core/src/storage/file_change_repository.rs`、`file_change_run_grant_repository.rs`、`agent_action_audit_repository.rs`
- Notification：`crates/core/src/storage/notification_repository.rs`、`storage/service/notifications.rs`
- Tree-scoped inputs：`crates/core/src/storage/agent_tree_resource_scope.rs`、`attachment_repository.rs`、`managed_artifact_repository.rs`、`storage/service/browser_downloads.rs`
- Bootstrap：`crates/core-server/src/transport/bootstrap.rs`

## 测试

- `crates/core/src/storage/migrations.rs` 内 canonical schema/fingerprint 测试
- `crates/core/src/storage/*_repository.rs` 及其 `tests/`
- `crates/core/src/storage/service/tests/`
- `crates/core/src/storage/service/tests/reconciliation.rs`
- `crates/core/src/storage/service/tests/trace_reconciliation.rs`
- `crates/core/src/storage/agent_command_session_repository/tests.rs`
- `crates/core/src/storage/automation_repository/tests.rs`
- `crates/core/src/storage/notification_repository.rs` 内测试与 `storage/service/tests/notifications.rs`
- `crates/core/src/storage/conversation_fork_repository/tests.rs` 的 FileChange/tree-resource policy 测试
- `crates/core-server/src/application/automation/scheduler/tests.rs`
- `crates/core-server/src/application/agent/tests/automation_turn.rs`
- Core Server 的 pending action、Provider transition、Command Session、image generation 和 collaboration 测试

## 变更检查表

- [ ] 修改 `canonical_schema.sql` 后同步 canonical version、fingerprint 和 fresh-schema 测试；若版本不再是 v35，同时更新本文当前快照。
- [ ] 明确旧数据库行为；没有经批准的迁移链时保持 reset-required。
- [ ] 新表/列定义 owner、FK、唯一键、索引、删除/保留和敏感分类。
- [ ] 跨表操作在一个 service 事务中完成，并有冲突/幂等测试。
- [ ] 外部工作在事务外执行，提交时重新校验 revision/CAS。
- [ ] 新文件目录采用受管根、staging、原子发布与 orphan cleanup。
- [ ] startup reconciliation 覆盖进程在每个提交边界崩溃的状态。
- [ ] FileChange schema 变更覆盖 Staged history、private audit/receipt、Run grant、fork 和 delete journal。
- [ ] Notification schema 变更保持 generic event/batch authority，并验证 Automation trigger 的原子 projection。
- [ ] 新 attachment/Artifact/download 授权从 Host-resolved Conversation/Project/tree scope 推导，覆盖跨树与普通 Conversation 负向测试。
- [ ] Automation schema 变更覆盖 occurrence/manual/nonterminal 唯一索引、父资源 trigger、Event/outbox 去重和 admission 崩溃窗口。
- [ ] fork/rewrite/delete/backup 行为同步更新相关领域文档。

## 当前限制

- 仅支持经过 fingerprint 和外键验证的 v34 → v35 升级，没有任意旧版升级链，也没有自动降级。
- 单连接 Mutex 设计偏向桌面本地一致性，不适合多进程或高并发服务端部署。
- 自动保留期限、跨设备同步、在线增量备份和用户级导出策略尚未形成统一公共契约。
- Automation 当前没有 history/Event/outbox retention 或物理 GC 公共流程；tombstone 与事件日志会随使用增长。
- Shared Notification event/batch 也没有统一的用户可配置 retention/GC；native delivery 最多尝试 5 次，失败耗尽后转为 suppressed，尚无独立修复 UI。
- Agent task tree 的 durable 资源共享当前以不可变 root identity 为边界；没有跨树转授权或细粒度成员级撤销协议。
- 物理文件和 SQLite 无法共享单个 ACID 事务，依赖 staging、原子改名和 reconciliation 收敛。
- SQLite FTS/索引损坏时需要重建；索引不可替代原始消息或 Archive。
