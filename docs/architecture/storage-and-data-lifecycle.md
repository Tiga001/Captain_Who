---
status: current
audience: developers
owner: engineering
last_verified: 2026-09-13
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

截至本次核验，当前唯一受支持的 canonical schema 是 **v47**（SQLite `PRAGMA user_version = 47`）：

- `STORAGE_SCHEMA_VERSION = 47`；
- canonical schema fingerprint 由 `migrations.rs` 中的编译期常量和测试固定；
- 空数据库在一个原子流程中建立完整当前 schema；
- v47 新增独立的本机 Token 元数据、请求去重账本和每日汇总表。精确匹配 v46 fingerprint 且外键有效的库可在单个事务内无损升级，只新增三表和统计起始时间，不回填观测、不改聊天/配置。失败整笔回滚；这不是重置，也不要求用户丢弃旧数据；
- v46 新增 `agent_workspace_run_bindings` 和 `agent_workspace_wake_bindings`，以不可变 JSON 保存文件夹 ID、别名、角色、配置路径、canonical 路径和目录实体身份；Run admission 与轨迹同事务提交，spawn/followup/结果 Wake 继承源 Run 或源 Wake。历史 fork 复制已保留回复的 Run 工作区绑定，但不复制 Wake 执行权；
- v45 把项目改为多文件夹模型：`projects` 不再保存 `path`，文件夹存放在 `project_folders`（每个项目恰好一个 `primary`，其余为 `auxiliary`，`path` 与 `alias` 在项目内唯一，随项目级联删除）。主文件夹仍是 Agent 的工作目录；
- 唯一原地升级路径为 exact v46 → v47：exact v45 及更早版本（包括 v34/v35/v42/v43/v44）仍一律返回 reset-required，不改写旧库；开发期不为单路径项目、聊天或运行历史写迁移；
- Run/Wake 模式冻结表和 `agent_prompt_preferences.context_profile` 与 v44 相同；新偏好默认 Full，旧 checkpoint 由版本校验直接拒绝；
- 其他旧版、未知版、非空未版本化或结构被篡改的数据库返回 `development_storage_schema_reset_required`，不自动重置。

版本号和 fingerprint 可能变化，维护时必须读取 `crates/core/src/storage/migrations.rs`，不得从本文复制常量到运行逻辑。发布说明可以记录版本，但架构文档应强调策略而非长期维护一张迁移历史表。

开发库重置前应先关闭应用并备份数据根；优先使用受管 `storage:reset-dev` 流程。不要只删除 `storage.sqlite` 而遗留 attachments、artifacts、spool 或 lock 文件。

`storage:reset-dev` 是显式丢弃历史的重建操作，始终新建 v47，不恢复 Conversation、Project（含旧的单路径项目）、本机 Token 统计或 Agent/runtime 历史。它从 exact current v47 及 exact v35–v46 保留 allowlisted 配置与凭据引用；v36–v47 还保留人机交互设置及 revision，v43–v47 保留全局协作开关及 revision，v44–v47 保留轻量/完整模式，旧版本该项默认 Full。Run/Wake 冻结策略属于运行事实，重置时清空。既有受限恢复选项也可从绑定 exact v33 fingerprint 的私有备份读取 allowlisted 设置，并把凭据转换为 reference。无法安全识别且含配置的旧库拒绝重置，不能用默认值默默替换模型配置。正常 v46 → v47 升级不使用此工具。

## 本机 Token 统计

`model_request_observation_repository::insert_observation` 在可嵌套 SAVEPOINT 内保存最终请求观测和独立统计。主 Agent、子 Agent 和压缩共用此边界；不累加 Run 的累计事件。`local_token_usage_requests` 只保留不透明请求 ID、北京时间日期和可选整数计数，不关联账号、会话、模型或内容，也没有 conversation 外键。`local_token_usage_days` 保留每日整数累计和缺失实际用量的请求数。删除聊天或清空计费记录不会删除它们；分支历史不会制造新的用量。

统计起始时间在 schema 创建/升级时固定。此前完成的观测即使迟到重放也不计入，不回填历史。OpenAI 缓存属于输入子集，Anthropic 输入按既有观测规则加上 cache read/write；output 已包含支持协议中的 reasoning，不再次累加思考字段。缺失完整实际用量记为未报告请求，不用估算值冒充准确零。每日和全历史计数使用 Rust u128 和十进制 TEXT/JSON 字符串，避免 SQLite/JavaScript 浮点精度损失。

`agent.getLocalTokenUsage` 只返回固定 `Asia/Shanghai` 时区、统计起始时间、请求日期范围内的每日总数、全历史总数/单日峰值/未报告请求数和今天用量。日期范围最多 3660 天。该接口无账号参数，不上传、不跨设备合并；账号退出或切换不改变统计。真源为 `local_token_usage_repository.rs`、`protocol/local_token_usage.rs` 和 TypeScript `localTokenUsage.ts`。

`agent_context_profile_run_policies` 以 `conversation_turn_traces.run_id` 为外键，`agent_context_profile_wake_policies` 以 `agent_wake_requests.wake_id` 为外键；两表只接受 `full | minimal`，禁止更新，随父记录删除。Run admission 与模式冻结同事务，Wake 入队继承来源 Run/Wake 的模式。它们不属于偏好表，也不能在设置保存时批量改写。

## 消息创建与重试归属

普通发送由 Host 的 `startConversationTurn` 在接纳事务中创建消息与 Run。Renderer 可乐观展示，不能同时把初始 user/assistant pair 排队写入存储；未建立 Run 的失败输入才通过独立新增接口保存，便于恢复输入、附件和错误。

`storage.upsertChatMessages` 是新增及幂等重试边界：同会话已存在的 ID 返回原持久化事实，不覆盖时间、正文、角色、运行状态或消息位置；与另一会话冲突的 ID 使整批事务回滚。保留首次事实也适用于已持久化的本地失败消息，后续状态更新走专用接口。agent/snapshot 原有不可变校验保持不变；Host Turn 接纳、流式与终态写入、编辑重发及分支各自的事务路径不受该新增接口影响。

此处的重复保存不能只返回原入参，否则前端会误以为迟到副本已成为权威记录。回执必须在同一事务读取/返回实际保留的记录，按请求顺序对应。该修复不改变 schema，不需要重置或改写旧聊天数据。相关最终模型请求回归见[上下文管理](./context-management.md)。

`saveChatMessageState` 不修改创建时间。对已有 Trace 的助手消息，回存的 `agentRunJson.runId` 必须等于已接纳的 Run；未确认启动的本地失败/取消或其他 Run 的状态不能覆盖该消息。身份匹配后仍复用现有终态围栏，保留正常状态回存与同 Run 迟到 running checkpoint 的处理；纯本地未接纳消息不受 Run 绑定限制。Renderer 启动失败/取消也只本地展示，再通过新增接口保存尚不存在的失败 pair。

成功完成的回答正文以 Host 提交的 `messages.content` 为准。终态提交、历史重建和 Renderer 终态显示都会移除 Timeline 中有 `streamId` 但没有 `traceSequence` 的临时回答流，并清空完成 Run 的 `messageStreamCheckpoints`；有 Trace 身份的过程说明完整保留，不按文字相同或前缀关系删除。迟到的 Renderer 快照不能覆盖已经完成的权威正文或复活临时流。取消、失败和等待审批不采用成功回答的流清理规则，以保留原有中断及恢复语义。

## 连续模型历史与不可变图像引用

当前 Trace v6 新增 `context_material`：保存首次发送的附件说明、Skill 完整说明与 Run World State，绑定原 Trace sequence、事件身份及模型日志。跨 Run、fork 与 child snapshot 复制这份已观察历史，不重建旧 Run 的运行授权。消息删除/编辑重发仍通过现有级联与 superseded 可见性规则裁剪历史。

模型日志只持久化图片的 attachment ID、MIME 与 SHA-256，不写入重复 base64。Host 在普通启动、上下文预览、压缩重建与恢复时，按同一会话可见消息归属读取附件并验证 MIME、长度和原始字节摘要；缺失、替换、跨会话或已被编辑替代的附件一律失败，不静默改用预览图。分支重绑定附件 ID 并复制原始文件，保留摘要；子快照不复制问答待办、Run 授权或用量。

已压缩前缀只额外保留有不可变图片引用的材料，避免文字摘要替代原始视觉输入；不会由此复活旧助手正文或纯文字 Run 状态。checkpoint v19 和恢复信封 v14 只支持本版本恢复；v19 的 `workspace.binding` 模型投影使用逐源 patch，旧检查点在版本入口拒绝，持久化 World State 完整 section 日志结构不变。Host 临时 `context_image_attachments` 字段不进入恢复信封 allowlist，图片 bytes 由 Host 重新加载并校验；既有检查点图片字段沿用原有保存策略，新增不可变引用跨重启保留。

## Conversation World State 请求日志

`conversation_world_state_records` 保存 canonical full/diff、可选精确模型请求边界及 `model_observed`。`conversation_world_state_request_commits` 以 conversation/run/request index 绑定准备快照和确认观察的前缀；无状态变化的请求也有幂等回执。请求准备与状态 append 在同一事务完成，确认观察是单独状态，不依赖只在请求终态记录的诊断 observation。压缩、分支、删除与重写同时处理精确 Trace 边界和回执生命周期。详见[上下文管理](./context-management.md)。

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

Automation 在 canonical schema v46 中使用当前通用通知表和自动化领域表，完整列、CHECK、索引和 trigger 仍以 DDL 为准：

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

## 向用户提问表组

独立设置、问题批次、不可变回应、回答投递和同步挂起记录分别持久化。创建提问在数据库事务内复核根身份、运行绑定与实时设置；提交或忽略通过 revision/CAS 和 submission identity 结算一次。正式提交与 pending delivery 原子写入，忽略没有 delivery。第 1 轮只持久化这些事实，暂停恢复和真实投递在后续轮次接入。详见[向用户提问](../subsystems/human-interaction.md)。

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

Bootstrap 大致执行：解析数据根与锁、打开/校验 canonical schema v46、构造 repositories/services、加载凭据 backend、MCP Server/Provider/Skills/Artifact Runtime、随后运行领域 reconciliation。持久凭据 backend 不可用或 credential reference 无法解析时必须保留公开配置并报告 `unavailable`/配置错误，不能把缺失凭据当作空值覆盖；只有实际需要该连接的运行应被阻断。

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

当前 v46 SQLite snapshot 只含模型/搜索 credential reference 与非秘密元数据，不含这些连接的当前 secret 字节；旧 schema 生成的历史备份仍可能含明文 Token/Key，必须继续按秘密材料保护。只恢复 `storage.sqlite` 不会恢复操作系统凭据，跨设备、跨系统账户、签名身份变化或凭据 backend 丢失后，界面可能显示凭据不可用，此时只能由用户替换或清除。未签名 macOS 开发构建的私有凭据文件位于数据根，因此“整根复制”仍会复制 secret，不能当作普通诊断包。

删除 SQLite reference、清除凭据或移除私有文件只表达应用层删除意图；文件系统、SSD、系统备份和操作系统凭据后端可能保留副本，产品不承诺安全擦除。怀疑泄露时应在 Provider 侧撤销或轮换凭据。

错误日志可包含 schema version、fingerprint、record identity、phase 和 recovery code，但不得打印消息正文、原始 Tool 参数、continuation 密文解密结果、密钥或私有绝对路径。

## 不变量

1. `canonical_schema.sql`、canonical schema v46 的 version 与 fingerprint 必须一致。
2. 所有非空非当前 schema 均 fail closed，只能通过显式开发库重置进入当前版本。
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

- [ ] 修改 `canonical_schema.sql` 后同步 canonical version、fingerprint 和 fresh-schema 测试；若版本不再是 v46，同时更新本文当前快照。
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

- 开发期只支持上述 exact canonical v42 单向升级，没有通用旧库迁移链或自动降级；其他旧库必须显式走受管 reset 流程。
- 单连接 Mutex 设计偏向桌面本地一致性，不适合多进程或高并发服务端部署。
- 自动保留期限、跨设备同步、在线增量备份和用户级导出策略尚未形成统一公共契约。
- Automation 当前没有 history/Event/outbox retention 或物理 GC 公共流程；tombstone 与事件日志会随使用增长。
- Shared Notification event/batch 也没有统一的用户可配置 retention/GC；native delivery 最多尝试 5 次，失败耗尽后转为 suppressed，尚无独立修复 UI。
- Agent task tree 的 durable 资源共享当前以不可变 root identity 为边界；没有跨树转授权或细粒度成员级撤销协议。
- 物理文件和 SQLite 无法共享单个 ACID 事务，依赖 staging、原子改名和 reconciliation 收敛。
- SQLite FTS/索引损坏时需要重建；索引不可替代原始消息或 Archive。

## 多文件夹执行与浏览边界

`AgentWorkspaceContext.folders` 是必填的 Host 字段。新根轮次从项目捕获当前集合；已开始的任务树、检查点、审批恢复和运行中的上下文预览沿用同一快照。准备期间配置若发生竞态变化，任务失败并要求重发，不能把旧 Skill 主目录与新运行工作区混用。根会话闲置/完成后的预览读取下一轮配置；子 Agent 终态预览继续沿用原任务树快照，与后续 Wake 一致。预览和实际请求、圆环及自动压缩共用 World State 投影。

默认相对路径、命令 cwd、搜索、`workspace_map` 和 Skill 发现仅针对主文件夹。`@workspace/<alias>/...` 在结构化路径参数中明确选择根；`./@workspace/...` 是主目录下的真实同名文件夹。不会改写 shell 命令正文。`workspace_only` 覆盖冻结并集，同时保留每个工具已有的越界、符号链接、硬链接及审批校验。选中根实体变化立即拒绝，无关辅助根离线不妨碍主目录。

历史文件/图片/Office 卡片向 Host 提交原始路径和 `assistantMessageId`。Host 用 `storage.resolveRunWorkspacePath` 按原 Run 的目录实体解析；不能从当前项目重猜旧别名。显式绝对/系统路径沿用原有 Host 读取和 reveal 语义。`workspace.binding` 只向模型显示别名、角色、可用性和寻址规则；真实路径与目录实体保留在 Host 状态。

Files 当前读取按 project + folder id 定位；历史读取再携带 assistant-message id，使用原 Run 解析目录。Git Review 单源快照冻结项目成员、目录实体和 Git 身份；读取及 mutation 均复验快照，目录移除、改绑或仓库变化使旧操作失效。LastTurn 可按所有 Git 源聚合，但仍只读取同一次 Run 的冻结文件记录；来源无法匹配时明确显示不可用信息，不能按新别名重新解释历史。Terminal 源选择使用创建时捕获的目录实体，验证后向原 PTY 发送 cd。以上页面选择属于 Renderer 内存状态，不新增数据库 schema。
