---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-10-11
---

# 第三轮：剩余同步存储适配入口

## 范围与核验基线

这是第三轮工作区的静态审阅快照：基于 `f073d86d3bd7b9c95e6ca4843a3df6895ebd5fcb` 加本轮未提交实现；最终交付提交以 [PROGRESS.md](PROGRESS.md) 为准。
它列出仍直接或经 `template_connection` 使用 `StorageState::connection()` 的生产入口及迁移边界，不表示第三轮已经完成全部存储调用迁移，也不是已发布版本承诺。

本次扫描覆盖 `crates/core/src`、`crates/core-server/src`，并复查其余 `crates/`。排除测试文件、测试模块、fixture 和 benchmark；`StorageState` 自身的内存库兼容分支单列。
Core Server 生产代码中没有找到直接取得这一旧 guard 的调用；其访问通过下列 Rust Core 服务转发。不能据此推断 Core Server 已没有同步数据库等待。

## 兼容方式与必须保留的约束

- [StorageState](../../../crates/core/src/storage/state.rs) 的旧 guard 与 `write_sync` / `write_async` 使用同一个 [FIFO admission gate](../../../crates/core/src/storage/execution.rs)、同一写连接与同一 mutex，不是另开写连接绕过队列。旧 SQL 仍在调用线程上执行；只拥有新 gate 不等于已迁移到专属 writer 线程。
- 新旧写入都在连接归还时检查未关闭事务、必要时回滚，并在确认提交后验证 durable journal 头再唤醒订阅。通知只是 wakeup，durable journal/cursor 和补拉才是事实来源；不能用 update_hook 或 commit_hook 代替提交成功。
- 旧 guard 当前只预留固定 16 KiB 的估算预算。它受同一个数量/FIFO约束，但该值不是其输入、历史、压缩结果或文件内存的完整估算；不能宣称所有旧路径已经实现精确字节上限。逐路径迁移时必须估算保留输入并区分事务外工作。
- 不得持有旧 guard 等待 `write_sync` / `write_async` / 另一次 `connection` 或 `read_connection`，也不能从 writer 回调递归调用 `StorageService`。同线程嵌套有明确拒绝；跨线程仍须靠锁顺序设计，不能依赖这一保护兜住任意环路。
- 第二轮 [阻塞执行器与生命周期](round2-blocking-lifecycle.md) 暂时继续承载准备、控制、维护、续租与终态的同步服务调用。对本轮已迁移事务，外层桥接等待内层 writer 是已知的两层接纳，不是可永久保留的目标架构；未逐个证明 future 取消、lease、新鲜权限重查及关机归属之前不能批量删除外层桥接。
- 第四轮前仍保留 execution_access / admission / RPC 全局保护和业务运行额度。后续迁移数据库执行位置不自动授权解除这些保护。

## 逐文件入口与后续边界

下列名称是本次工作区静态提取后逐文件检查的快照。内联 repository 调用可能继续委托其他函数；此处按取得 guard 的服务函数归属，不将下游每个 SQL 算作新入口。包装函数和 helper 的特别关系另行注明。

### [agent_collaboration_run_policies.rs](../../../crates/core/src/storage/service/agent_collaboration_run_policies.rs)

入口：`load_active_conversation_turn_identity`、`load_agent_collaboration_run_policy`、`load_active_agent_collaboration_run_policy`。

读取冻结 Run 策略与当前 Turn 身份，当前仍以 writer guard 保证观察顺序。候选只读迁移须确认每个 repository 只读；多 SQL 使用同一短快照；执行权限仍在原最终 admission 边界重查。

### [agent_collaboration_settings.rs](../../../crates/core/src/storage/service/agent_collaboration_settings.rs)

入口：`load_agent_collaboration_settings`、`update_agent_collaboration_settings`。

设置读取和 revision CAS 更新共用旧接口。读取可拆到读池；更新应整体进入短写事务，保留 revision 冲突和配置生效范围。

### [agent_context_profiles.rs](../../../crates/core/src/storage/service/agent_context_profiles.rs)

入口：`load_agent_context_profile_for_run`。

读取 admission 时冻结的 context profile。候选只读迁移应保持 Run 身份及删除后的 None 语义，不据旧快照生成新权限。

### [agent_templates.rs](../../../crates/core/src/storage/service/agent_templates.rs)

入口：`template_connection`。

经 `template_connection` 间接使用的入口：`create_agent_template`、`list_agent_templates`、`get_agent_template`、`list_project_agent_templates`、`get_project_agent_template_by_machine_key`、`update_agent_template`、`set_agent_template_enabled`、`delete_agent_template`、`set_agent_template_project_assignment`、`resolve_template_for_spawn`。

template_connection 是统一错误转换入口，下面另列间接使用它的公开方法。模板写入和 revision CAS 不能拆开；resolve_template_for_spawn 仍需 model credential coordinator → 数据库的锁顺序、模板与模型配置同一快照，凭据 I/O 在数据库 guard 外。

### [agent_workspaces.rs](../../../crates/core/src/storage/service/agent_workspaces.rs)

入口：`resolve_run_attachment_file`、`load_agent_workspace_for_run`、`load_agent_workspace_for_wake`、`load_run_workspace`。

读取冻结 workspace、Turn 身份及附件权限。resolve_run_attachment_file 已先释放 guard 再调用附件库服务；迁移时保留完整身份快照与路径验证，不在持连接期间递归调 StorageService。

### [attachment_image_cache.rs](../../../crates/core/src/storage/service/attachment_image_cache.rs)

入口：`cleanup_model_image_cache`。

清理时先读取引用路径，释放 guard 后处理缓存文件。可将引用集合查询移到只读快照；不能因过时集合删除刚发布的文件，沿用启动维护/发布协调边界。

### [attachment_imports.rs](../../../crates/core/src/storage/service/attachment_imports.rs)

入口：`cleanup_attachment_imports`。

cleanup_attachment_imports 读取数据库中受保护的导入引用，后续遍历/读取/删除在 guard 外。迁移查询时保留导入生命周期及 grace period，不把整个目录清理塞进 writer。

### [attachments.rs](../../../crates/core/src/storage/service/attachments.rs)

入口：`prepare_conversation_turn_rewrite_attachments`、`attachment_conversation_id`、`load_attachment_image`、`resolve_attachment_file`、`save_input_attachments`、`rebind_input_attachment_ids`、`store_agent_run_guidance_with_attachments`、`load_input_attachments`、`load_context_image_attachments`、`build_attachment_library_context_inner`、`cleanup_orphan_attachment_files`、`cleanup_new_attachment_files`。

包含附件身份读取、库范围、导入登记和清理。prepare/save/guidance 主复制路径已有显式 drop(connection) 后文件准备，再事务中最终校验与登记。后续只迁移 SQL 区段；附件与 guidance 归属登记保持一个事务，失败只清理本操作文件。只读多查询要同一快照，授权读取不能用旧快照跳过最终判断。

### [browser_data.rs](../../../crates/core/src/storage/service/browser_data.rs)

入口：`load_browser_preferences`、`save_browser_preferences`、`register_browser_history`、`update_browser_history_metadata`、`list_browser_history`、`delete_browser_history`、`summarize_browser_owned_data`、`clear_browser_owned_data`。

浏览器设置、历史和统计的同步领域服务。只读统计/列表需核实跨 SQL 一致性；偏好 CAS、批量删除及清理继续整体提交，保持既有计数及冲突返回。

### [browser_downloads.rs](../../../crates/core/src/storage/service/browser_downloads.rs)

入口：`load_browser_download_settings`、`save_browser_download_settings`、`register_browser_download`、`list_browser_downloads`、`load_browser_download`、`clear_browser_download_history`、`authorize_browser_download_input`。

下载设置、登记、历史及输入授权。把文件检查与SQL区段区分；只读迁移不能绕过已完成下载、文件身份、内容和授权规则；写入保留 claim/登记边界。

### [child_agents.rs](../../../crates/core/src/storage/service/child_agents.rs)

入口：`resolve_child_agent_wake`、`resolve_running_child_agent_wake`、`validate_agent_wake_execution_fence`、`resolve_active_child_agent_wake_by_identity`。

这里剩余的是 Wake 解析和最终执行 fencing，不是已迁移的子 Agent 创建事务。权威校验保持新鲜时钟、原 claim token、tree generation、冻结 reasoning/model 身份；不能把缓存或旧读快照作为执行许可。

### [compaction.rs](../../../crates/core/src/storage/service/compaction.rs)

入口：`project_context_compaction_prefix_for_model`、`conversation_requires_context_adaptation`、`get_active_context_compaction_summary`、`save_model_request_observation`、`record_context_compaction_receipt`、`list_provider_transition_receipts`、`provider_transition_failed_attempt_count`、`prepare_context_compaction_prefix`、`prepare_context_compaction_prefix_if_current`、`commit_context_compaction_prefix`、`commit_context_compaction_prefix_if_current`、`commit_context_compaction_prefix_with_receipt_if_current`、`commit_provider_transition_with_receipt_if_current`、`rollback_context_compaction_summary`。

上下文前缀准备、压缩回执、模型切换及回滚仍走旧连接。需拆分读取/解码与完整提交：prefix、active summary、history revision、Provider identity、usage/receipt 必须原子验证，不可将 prepare/commit 改成互不关联的独立写。

### [file_change_run_grants.rs](../../../crates/core/src/storage/service/file_change_run_grants.rs)

入口：`create_pending_file_change_run_grant`、`get_active_file_change_run_grant`、`get_file_change_run_grant_for_pending_action`、`revoke_active_file_change_run_grant`、`retire_pending_file_change_run_grant`、`revoke_nonterminal_file_change_run_grants`、`list_active_file_change_run_grant_records`、`list_pending_file_change_run_grant_records`。

运行期文件变更授权的读取、创建、撤销与列表。不得简单按 get/list 名称把授权执行决策移到旧快照；创建/撤销保留身份 CAS 和终态规则。

### [file_changes.rs](../../../crates/core/src/storage/service/file_changes.rs)

入口：`create_agent_file_change`、`get_agent_file_change`、`get_agent_file_change_for_owner`、`get_agent_file_change_for_source_call`、`list_agent_file_changes_for_run`、`list_agent_file_change_runtime_states_for_run`、`list_agent_tool_results_for_run`、`list_agent_command_results_for_run`、`settle_unresolved_agent_file_changes_for_run`、`save_agent_file_change_progress`、`update_agent_file_change`、`transition_agent_file_change`、`get_agent_file_change_operation`。

文件变更事实、工具结果读取及进度/终态更新。写入须保留 operation identity、状态 CAS 和 pending action 的事务关系；大 JSON 构造/解码放到连接外，读取多表要一致快照。

### [image_generation.rs](../../../crates/core/src/storage/service/image_generation.rs)

入口：`claim_image_generation_execution`、`prepare_image_generation_artifact`、`finalize_image_generation_execution`、`inspect_image_generation_execution`、`list_interrupted_image_generation_executions`、`resolve_published_generated_artifact_input`。

图片生成执行 claim、Artifact prepare/finalize、恢复列表及发布引用解析。必须保留 claim/幂等/Artifact 发布关系与真实计费语义；文件工作不进入 writer；inspect/list 需逐函数确认只读。

### [lifecycle.rs](../../../crates/core/src/storage/service/lifecycle.rs)

入口：`open_internal`。

open_internal 在对外接纳请求之前做文件变更过期清理和中断恢复。不是常规并发热路径；仍应在后续维护迁移中拆 SQL 与文件清理，不能将启动恢复误当纯查询迁到读池。

### [manual_compaction.rs](../../../crates/core/src/storage/service/manual_compaction.rs)

入口：`record_manual_context_compaction_observation_and_usage`、`claim_manual_context_compaction`、`get_manual_context_compaction`、`list_manual_context_compactions`、`update_manual_context_compaction`、`record_manual_context_compaction_usage`、`commit_manual_context_compaction_if_current`。

手动压缩的 claim、运行状态、用量与最终 prefix 提交。claim 不是只读 poll；保持 operation revision/幂等回执和关联用量的一次性语义。

### [provider_transitions.rs](../../../crates/core/src/storage/service/provider_transitions.rs)

入口：`can_send_with_provider_protocol`、`commit_compatible_provider_transition_terminal`、`get_provider_transition_terminal_record`、`list_provider_transition_terminal_records`。

预检包含多个身份查询，最终转换关联模型、revision 与 receipt。预检可一致快照，但不能替代 commit 时重查；terminal transition 整体进入写通道。

### [trace_reconciliation.rs](../../../crates/core/src/storage/service/trace_reconciliation.rs)

入口：`reconcile_orphaned_in_progress_conversation_turn_traces`。

孤儿 InProgress Trace 终态化属于恢复写事务，不能作为后台只读扫描；需保留 startup/action/claim 判定及同步可见性，与第四轮运行所有权协议一起验证。

### [turn_diffs.rs](../../../crates/core/src/storage/service/turn_diffs.rs)

入口：`initialize_agent_turn_diff`、`record_agent_turn_file_change`、`load_latest_agent_turn_diff`、`load_agent_turn_diffs_for_messages`。

Turn diff 初始化、文件变更记录和聚合读取。保留身份、单操作幂等及记录事务；读取聚合用短快照，大内容解析放在释放快照以后。

## 已识别的重工作边界

1. [Archive 准备模块](../../../crates/core/src/storage/conversation_history_archive_repository/preparation.rs) 已将两个服务保存入口的文件读取、UTF-8 分块、SHA256、zstd 压缩及 FTS 文本准备移到取得 writer 之前。File 准备两次扫描核验完整 hash/byte/char/chunk 身份，提交使用同一份冻结内容；内存输入借用原 String，不复制大正文。队列估算包含 FTS 全文、压缩 chunks 及元数据，准备也按预算拒绝过大输入。writer 内重查幂等/消息身份并把 blob、chunks、FTS 作为同一事务提交。SQLite FTS5 的实际分词及 SQL 写入仍属于事务成本，因此大 Archive 仍可能占用 writer，不能宣称事务耗时固定。原 repository `store_archive` / `store_archive_file` 保留兼容 API；当前生产服务入口使用显式锁外 prepare，直接 repository 调用仍要求调用者自行管理连接占用范围。
2. [Managed Artifact](../../../crates/core/src/storage/service/managed_artifacts.rs) 授权读取曾在持写连接时执行 `read_to_end` 与 SHA256，本轮收口按“短快照取得授权及不可变记录 → 释放连接后读取/哈希 → 新快照重新授权并比较完整记录”修复。最终以该文件和交付测试为准；不得通过单纯提前 drop 放松撤销/删除规则。
3. [附件服务](../../../crates/core/src/storage/service/attachments.rs) 的重写准备、输入附件保存、Guidance 附件登记已经先释放 guard 再复制/缓存文件，并在最后事务重查身份。保留这些正确分段；不能因为还出现 `connection()` 就把所有文件准备一起迁入写线程。旧完整历史/模型上下文仍有持连接解码、JSON 解析及 FTS/Archive 计算，需要独立 snapshot/materialization 设计。
4. [配置服务](../../../crates/core/src/storage/service/settings.rs) 和 [模型投影](../../../crates/core/src/storage/service/model_projection.rs) 有 credential coordinator、兼容配置迁移与 staging/cleanup journal。当前 `load_model_settings_snapshot` 已核实为短读事务，服务使用读池；配置写入经专属 writer，凭据后端调用位于数据库 guard 外。它们已从剩余入口清单移除，后续保持 coordinator → 数据库的锁顺序与 staging/cleanup 失败恢复，不能从 writer 回调反向获取 coordinator。

## 已迁读池但仍持快照解码的具体入口

以下路径已退出 writer mutex 竞争，不再属于上面的旧 guard 清单；仍会延长只读连接占用及 WAL 快照寿命。它们是明确的后续 materialization 任务，不能把“使用读池”报告成全部计算已经移出数据库占用区。

| 服务入口                                                                                                                                                             | 当前仍在读快照内的工作                                                                                                                                                                                                                                                                                                                                                 | 后续拆分边界                                                                                                                                                                       |
| -------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `load_conversations`、`load_conversation_views`、`load_conversation`、`load_conversation_view`、`load_conversation_observer_snapshot_page`                           | [chat_repository](../../../crates/core/src/storage/chat_repository.rs) 的 `list_active_conversations` / `get_active_conversation` / 分页变体读取消息后，经 `list_messages_in_scope` 解析和重建 usage、终态、交互 JSON；服务继续执行 lineage / guidance 投影。消息正文当前是 SQL TEXT，本体没有 zstd 解压，不应混写为全部消息解压。附件预览文件读取已经在释放快照以后。 | 同一快照采集原始消息、usage、终态、交互、lineage 与 revision；释放后执行纯投影。分页和未分页语义分别保留，不得把既有完整读取偷偷截断。                                             |
| `read_compacted_conversation_history`                                                                                                                                | [compacted history](../../../crates/core/src/storage/conversation_history_repository/compacted.rs) 的 `read_compacted_snapshot` 在原快照中读取被摘要覆盖的前缀，并调用 `get_log_for_message` / `get_trace_for_message`；包含模型上下文 zstd 解压、hash 核验、JSON 解析、Trace 前缀验证、消息映射和搜索正文构造。                                                       | 先采集同一 source/revision 对应的压缩模型上下文、Trace 原始行和投影依据；释放后完整解码与验证。不能为了早释放而混用不同版本前缀或用审计预览代替模型上下文。                        |
| `search_conversation_history`、`conversation_history_around`、`conversation_history_range`、`conversation_history_tool_exchange`、`read_conversation_history_record` | [conversation_history_repository](../../../crates/core/src/storage/conversation_history_repository.rs) 的 `read_record` / `get_tool_exchange` 在查询阶段解析、序列化 Trace JSON；服务的 `project_history_preview` / `project_history_record` 继续在同一快照读取并应用模型投影。FTS 查询及 snippet 计算仍是 SQLite 查询的一部分。                                       | 批量采集匹配项和投影事实，再锁外解析与渲染；保留记录顺序、范围、最大条数及消息身份，不用另一次快照拼接原记录。                                                                     |
| `find_conversation_history_archive_match_char_offset`、`read_conversation_history_archive_page` 及其包装读取                                                         | [Archive repository](../../../crates/core/src/storage/conversation_history_archive_repository.rs) 的 `read_archive_page` 一边遍历 chunk rows 一边 zstd 解压和 UTF-8/字符分页；`find_archive_match_char_offset` 经 `read_complete_archive_content` 解压完整内容、校验 hash，再做忽略大小写的定位，仍处于服务短读事务内。                                                | 在快照内取得 descriptor 与本次需要的压缩 chunks，释放后解压/定位。页面读取目前查询全部 chunk rows 后筛选，需要单独验证按区间 SQL 选择的 UTF-8 与页游标等价性；不得省略完整性检查。 |

以上不得与已完成的 [history_snapshot](../../../crates/core/src/storage/service/history_snapshot.rs) 主路径混淆：它已在同一快照取得 `StoredConversationTraces`、`StoredModelContextLogs` 和 summary validation 数据，然后 commit / 释放连接，再执行 Trace 与模型上下文 decode、summary 校验及附件预览。缓存命中时的大量 presentation overlay 也已经锁外执行。

另外，已迁专属 writer 的 `delete_unreferenced_exact_conversation_tool_result` 仍由 repository 在完整 compare-and-delete 事务中计算输入 hash、核对被引用状态。可以后续冻结输入并锁外预计算 hash，但必须在原事务内保留身份、内容摘要及未引用状态校验；这不是当前提交已完成的优化。

## StorageState 自身与扫描复核

`StorageState::read_connection` 对 `:memory:` 使用旧单连接适配器，避免悄悄变成互不相干的多个内存数据库；磁盘库使用读池。这不是遗漏的生产磁盘读池迁移。测试专用 `configure_read_connections_for_test` 不计入本清单。

复核候选命令（覆盖链式换行；结果必须排除 inline `#[cfg(test)]` 模块，并按真实调用语义确认，不能仅按文件名统计）：

```sh
rg -n -U '\.connection\s*\(\s*\)' crates --glob '*.rs'
rg -n 'template_connection' crates/core/src/storage/service/agent_templates.rs
rg -n 'write_sync|write_async|read_connection' crates/core/src/storage/service
```

本页不代替持续漂移检查；每次迁移需同步删除对应旧入口，并核验同名包装方法及调用者。核验时不访问真实用户数据库、不触发模型、不启动或停止现有软件。此次仅做静态审阅；性能数值、运行测试与未完成范围见本轮统一记录，不能从这张清单推导吞吐提升。
