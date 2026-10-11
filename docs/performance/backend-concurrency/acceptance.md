---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-10-11
---

# 后端并发优化：测量与正确性验收

本页规定比较方式和后续轮次门槛，补充 [资源与事务契约](./architecture-contracts.md)。源码核验基线为
`aac5fa6b`；第一轮的执行命令、通过/失败、原始结果和未测范围由 [PROGRESS.md](./PROGRESS.md) 与
[results.md](./results.md) 记录。**下列“已有测试”表示已定位到代码，不表示本轮已经运行或通过。**

## 1. 性能比较必须保持相同工作量

每次运行记录：源码 commit、工作区差异指纹、新增测试文件指纹、lockfile、schema、SQLite 实际版本与 PRAGMA、构建
模式、机器/系统、线程与队列/业务容量、夹具 seed、历史大小、输出速率、背景查询、冷/热/恢复状态和结果文件位置。
debug/release 分开报告；固定临时磁盘 fixture，不读写真实用户数据库，不发真实计费模型请求或对外消息。

1. 首先记录提供负载，再按固定时间表发出；不能只在上一请求完成后发下一请求，从而隐藏排队。
2. 分别计数 offered、admitted、started、durably committed、completed、rejected、timed out、cancelled、unfinished、
   retry。重试既算工作量又保留原操作身份，不能只保留最终成功样本。
3. 记录从计划发出时间和实际发出时间到结束的延迟；前者暴露压测驱动器自身排队。
4. 同时报告吞吐与固定负载下的延迟分布；容量变大、拒绝减少但 backlog 一直增长不是吞吐提升。
5. 效率比较保持相同事实和输出字节：允许无损合并传输，但不能少保存 Trace、缩短历史、跳过授权或省略终态。
6. 每个进程用本地单调时钟测跨度，跨进程用 request/operation 关联，不能相减不同进程的 `Instant`。
7. 指标使用有限静态类别，不把 Conversation/Run ID、消息正文、路径、密钥放入聚合标签；公平性原始夹具 ID 只属于
   测试数据。诊断必须可关闭且有界，记录开启诊断的开销。

## 2. 时间点和统计口径

| 阶段             | 起止点                                                             | 能说明什么；不能说明什么                                                    |
| ---------------- | ------------------------------------------------------------------ | --------------------------------------------------------------------------- |
| 用户动作前置     | handler 收到动作 → 前置 metadata/message flush 完成                | Renderer 等待；普通审批与子 Agent 投影审批分开，不能把 RPC 时间冒充点击时间 |
| Host 传输        | Main 调用开始 → 序列化/写入完成、Core Server 响应到达              | 序列化、pipe backpressure、端到端 RPC；不能单独证明 Core Server 排队来源    |
| Core Server 排队 | 请求 admission → owner/fence/执行容量就绪                          | 队列与必要顺序等待；记录深度、字节、最老等待和拒绝                          |
| 准备             | 准备开始 → 准备结果可提交                                          | 历史、上下文、文件准备；不得把外部 Provider 首 token 等待混入               |
| 存储             | 入队/等待连接 → 获得连接 → 事务开始 → commit 返回                  | 区分排队、持锁、事务和 commit；持锁时间不等于纯 SQL 时间                    |
| 持久接纳         | 用户动作/基准请求 → admission/决策事务成功返回                     | 接纳语义已落盘；不是工具执行结束                                            |
| 执行             | admission → worker 实际开始 → 外部动作/模型完成 → durable terminal | 单独观察接纳后等待、真实执行和结算，不通过提前 ACK 隐藏它们                 |
| 展示应用         | Renderer 收到回执/事件 → 数据投影应用（必要时 paint）              | 展示链尾端；本轮未覆盖 UI 时明确缺失，不从 Core Server 测试推导             |

延迟报告至少含样本数、p50/p95/p99、max、错误与 unfinished。样本很少时 p99 只是离散样本上界附近估计，不宣称可靠
尾部结论；关键分布可用至少 10,000 个有效观测或重复批次与置信分析支持，不能把几十个样本包装成稳定 p99。
有界直方图若给 bucket 上界，必须写出精度/溢出含义，不能称精确原始分位数。

资源报告包含 CPU、RSS、线程数、runtime timer lag、磁盘写入、WAL 大小、连接等待/持有、队列容量、最长等待、每
Conversation/root 完成量与尾延迟。峰值 RSS 是该进程生命周期统计时，不把多场景累计 peak 当成单场景净增长。
OS 不提供可靠写入计数时标为 unavailable，不能用 RPC payload bytes 代替真实磁盘写入。

## 3. 工作负载与暂定目标

目标测试矩阵使用 1/5/10/20/50 个运行单元，**根 Agent 也占运行单元**。50 个 child 加一个 root 是 51，不能绕过现有
业务容量。独立 Conversation、单根多 child、多 root 竞争分别报告；相同进程数和相同提供速率下比较。

| 维度      | 必须区分的场景                                                    |
| --------- | ----------------------------------------------------------------- |
| 生命周期  | 冷启动、恢复、热稳态；构建时间不计为交互延迟                      |
| 历史      | 短历史、长历史、超过缓存预算；模型/Trace 内容和字节固定           |
| 正常负载  | 固定流式输出、正常后台查询、按确定时间表注入普通发送/审批/停止    |
| 突发      | 同时待批动作、多个不同 root；记录拒绝和恢复时间                   |
| 慢动作    | 1/10/30 秒受控工具或准备；观察 A 慢时 B 审批/停止是否随之增长     |
| 长期/背压 | 后续综合验收 10–30 分钟稳态、突发后排空、慢消费者、断连重连       |
| 故障      | commit/磁盘写失败、ACK 丢失、租约到期、取消/删除/撤销、关停及重启 |

这些是六轮共同验收矩阵；第一轮实际覆盖和缺失必须在结果页逐格列出，不能由一个 storage/dispatch 微基准宣称全部
已测。可控 Provider/Tool 用于隔离本地性能，不用真实云模型延迟解释本地尾部。

| 暂定指标                    | 暂定门槛                                      | 测量边界                                                        |
| --------------------------- | --------------------------------------------- | --------------------------------------------------------------- |
| 1–20 活跃运行单元的控制响应 | p95 ≤ 250 ms；p99 ≤ 1 s                       | 点击/操作收到 → 持久决策或具有原有语义的控制回执；底层 RPC 另列 |
| 50 活跃运行单元的控制响应   | p95 ≤ 500 ms；p99 ≤ 2 s                       | 同上，不能靠饿死普通写或某个 root 换取                          |
| 普通短历史发送              | p95 ≤ 500 ms                                  | 请求 → durable admission；执行开始延迟另列                      |
| 隔离                        | 慢 A 不导致无关 B 的控制延迟随 A 耗时线性增长 | 同时报告 DB/owner/准备等待，不能只测空 handler                  |
| 稳定性                      | 完成速率能承受提供速率，backlog 不持续增长    | CPU/RSS/线程/WAL 有界，突发后能排空                             |

以上是**待实测校准的目标，不是当前已达成结果或产品 SLA**。某个 RPC 的现有完成语义若要求外部动作结束，应另设
执行耗时指标；不能为了达到表中数字把“已完成”改成“已排队”。第一轮先解释当前是否满足及偏差来源，后续比较必须
相对包含已提交审批与 World State 优化的基线。

## 4. 正确性回归映射

表中测试名称为当前源码中的可搜索名称。已有局部测试不等同于迁移后整个调用链已获证明；本轮实际运行清单另见结果。

| 契约/竞态                                            | 已有测试与位置                                                                                                                                                                                                                                                                                                                                                                                                                                                       | 迁移前仍需补足                                                                                          |
| ---------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| 相同 Conversation 启动只一赢家；陈旧释放不伤当前占用 | `concurrent_admission_has_one_winner_and_failed_preparation_can_release_it`、`durable_occupancy_survives_restart_without_a_resident_runtime_and_requires_exact_owner`：[occupancy.rs](../../../crates/core-server/src/application/agent/turn_executor/occupancy.rs)                                                                                                                                                                                                  | 测试目前直接调用 reservation；第 4 轮再覆盖两个真实 start/prepare/final commit、失败及重试              |
| 审批可绕过无关保存，不能绕过自己或全局操作           | `approval_bypasses_unrelated_scoped_writes_but_waits_for_its_own_snapshot`、`approval_respects_global_mutations_even_when_the_dto_contains_an_owner`：[审批分派测试](../../../crates/core-server/src/transport/bounded_dispatch_approval_tests.rs)                                                                                                                                                                                                                   | 第 2/4 轮加真实慢准备 A + B 真实审批/停止的全链路；sleep handler 仅证明 dispatch 围栏                   |
| 同一对话不同 Run 别名与未知 owner 保守处理           | `different_run_ids_in_one_conversation_share_the_priority_fence`、`unknown_approval_owner_falls_back_to_the_admission_time_write_fence`：同上                                                                                                                                                                                                                                                                                                                        | owner scope 拆分后覆盖错 DTO、owner 在执行前变化、跨资源和新资源集合                                    |
| 重复审批只发放一次执行权；停止挡住迟到决策           | `command_approval_preserves_checkpoint_and_claims_exact_identity_once`、`concurrent_command_approval_grants_dispatch_once_and_respects_tree_stop`：[notifications.rs](../../../crates/core/src/storage/service/tests/notifications.rs)                                                                                                                                                                                                                               | 第 2/4 轮真实执行回执计数，approve/reject/cancel 与删除/撤销各方向竞态                                  |
| 通知失败必须回滚审批/终态                            | `command_approval_notification_failure_rolls_back_dispatch_authority`、`invalid_notification_rolls_back_the_terminal_message_and_trace`：同上                                                                                                                                                                                                                                                                                                                        | 第 3 轮独立读连接只能看到已提交状态；commit 失败与 rollback 不发成功通知                                |
| 权限在最终 admission 仍有效                          | `execution_access_expiring_at_storage_commit_rolls_back_messages_trace_and_binding`、`permission_revoked_after_precheck_is_blocked_atomically_before_humanroot_admission`：[automation_turn.rs](../../../crates/core-server/src/application/agent/tests/automation_turn.rs)；`execution_access_fences_both_revision_and_account_epoch`：[execution_access.rs](../../../crates/core-server/src/application/agent/execution_access.rs)                                 | 第 2/4 轮锁外慢准备，撤销/重新登录 epoch ABA 与最终 commit 的真实交错                                   |
| 历史快照与最终 CAS                                   | `history_snapshot_admission_rejects_trace_only_mutation_transactionally`、`history_snapshot_recreated_conversation_cannot_reuse_old_epoch`、`history_snapshot_version_cannot_cross_storage_instances`：[history_snapshot.rs](../../../crates/core/src/storage/service/tests/history_snapshot.rs)                                                                                                                                                                     | 第 3/4 轮 read pool 的多查询 snapshot/cursor 一致性，准备与重写/模型迁移/压缩竞争                       |
| commit ACK 丢失、游标失效与终态不可回退              | `commit_ack_loss_and_reopened_database_rebuild_without_duplicate_items`、`rollback_mid_append_drops_cursor_and_retries_from_the_durable_prefix`、`concurrent_terminal_commit_never_reopens_a_settled_turn`：[trace_publication_tests.rs](../../../crates/core/src/storage/service/trace_publication_tests.rs)；`external_terminal_commit_fences_a_waiting_publication`：[外部连接测试](../../../crates/core/src/storage/service/trace_publication_external_tests.rs) | 第 3/5 轮真实 writer/reader、通知、进程断开/重启与前端 checkpoint 迟到组合，不仅存储函数重放            |
| claim 恢复不把未证明输入算完成                       | `workflow_execution_recovery_does_not_complete_unproven_claims`：[组织执行测试](../../../crates/core/src/storage/workflow_execution_repository/tests.rs)；`manager_repeats_recovery_after_a_fresh_restart_lease_expires`：[dispatcher tests](../../../crates/core-server/src/application/agent_dispatcher/tests.rs)                                                                                                                                                  | 第 2/4 轮 Preparing 在队列中租约到期；A 终态触发 recovery 与 B 的 claim→Trace 空窗；过期 token 禁止提交 |
| 根停止与审批恢复/关停交错                            | `stop_winning_approval_wait_cas_settles_immediately_without_lease_recovery`、`shutdown_rescans_an_approval_that_resumes_after_its_first_snapshot`、`stopped_tree_recovery_with_uncertain_external_action_becomes_outcome_unknown`：同上                                                                                                                                                                                                                              | 第 4 轮 root generation 与并行 spawn/followup 的最终事务边界                                            |
| 删除必须等副作用可证实结算                           | `conversation_deletion_rejects_unsettled_effects_without_a_project`、`successful_conversation_deletion_tombstone_rejects_stale_file_effect_context`、`project_deletion_timeout_preserves_late_command_observation`：[terminal_events.rs](../../../crates/core-server/src/application/agent/tests/terminal_events.rs)                                                                                                                                                 | 第 2/4 轮 Preparing 与删除、effect 注册与标记竞争；排空期间其他 Conversation 控制不被全局锁阻塞         |
| shutdown 不丢已开始写入，不启动等待中的 handler      | `shutdown_joins_running_mutation_and_responds_to_every_cancelled_queue_entry`、`shutdown_does_not_start_handlers_still_queued_in_the_shared_blocking_pool`：[bounded_dispatch.rs](../../../crates/core-server/src/transport/bounded_dispatch.rs)                                                                                                                                                                                                                     | 第 2/3 轮新的专属执行器、写通道关闭、lease/terminal drain 及超时恢复                                    |
| 每 Agent 单 Turn、有限容量和进展                     | `one_hundred_wakes_for_one_agent_never_overlap_and_all_settle`、`one_hundred_agents_respect_limit_fifty_fifo_and_leave_no_ghost_activity`：[dispatcher tests](../../../crates/core-server/src/application/agent_dispatcher/tests.rs)                                                                                                                                                                                                                                 | 现有 fake store/executor 不能证明真实 SQLite+准备下 tail/fairness；第 4/6 轮多 root 固定负载实测        |
| 子 Agent 真正端到端恢复/停止                         | `process_start_dispatcher_recovers_a_queued_child_without_a_new_root_turn`、`user_root_run_cancellation_stops_running_and_queued_descendants`：[collaboration_harness.rs](../../../crates/core-server/src/application/agent/tests/collaboration_harness.rs)                                                                                                                                                                                                          | 第 4/6 轮并行 scope 改造后重跑，并加入部分断连、持续输出与控制交错                                      |
| 等待交互恢复仍属于同 Run                             | `committed_unclaimed_sync_answer_resumes_after_host_restart`、`stopped_sync_wait_after_host_restart_rejects_late_submission_without_sampling`：[human_input.rs](../../../crates/core-server/src/application/agent/tests/human_input.rs)                                                                                                                                                                                                                              | 第 5 轮移除 Renderer 前置保存前，覆盖流中/问答/审批全部恢复组合                                         |

测试矩阵必须保留“已经存在”“本轮已运行”“本轮通过”“待补”四种区别。当前缺陷可以用诊断基准或显式 ignored 的
已知失败用例复现，但需说明原因和后续门槛；不得悄悄破坏默认测试套件，也不能把跳过当通过。

## 5. 可复用基准的适用范围

| 现有基础                                                                                        | 能证明的范围                                                       | 不能独立证明的范围                                         |
| ----------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ | ---------------------------------------------------------- |
| [dispatch benchmarks](../../../crates/core-server/src/transport/bounded_dispatch_benchmarks.rs) | lane 排队/执行分解、审批隔离的局部效应                             | 真正用户点击、慢准备、外部副作用、磁盘事务全链路           |
| [Trace benchmark](../../../crates/core/src/storage/service/trace_publication_benchmark.rs)      | 真实 StorageService、增长历史和多线程增量发布的 CPU/锁/commit 成本 | 审批/停止/UI 回存以及整个多 Agent 调度                     |
| [outbound benchmarks](../../../crates/core-server/src/transport/outbound_benchmarks.rs)         | 有界输出、序列化、控制回执传输及内容一致性                         | 一定负载下真实 Core Server 准备与数据库争用                |
| [多 Agent release gate](../../operations/multi-agent-release-gate.md)                           | 已定义的协议、领域与恢复门禁                                       | packaged UI 或生产真实 Provider 性能，除非该次结果明确包含 |

本轮新增磁盘/真实 handler 基准应叠加这些基础，不能用一个合成 UPDATE 或 sleep 基准替代全部验收。具体 runner、
命令、场景覆盖和原始输出以本轮实际交付为准。

## 6. 不予验收的情况

- 只有队列等待指标下降，输入到 durable ACK/实际执行的尾延迟没有改善或退化。
- 提高队列/线程/业务上限后短时不报 overload，但稳定完成率不变、积压或内存/WAL 持续增长。
- 少处理消息、跳过模型历史/校验、降低落盘可靠性，或把“完成”改成“接纳”却仍使用原字段。
- 只统计成功请求、排除 timeout/cancel/unfinished；不同数据量、构建模式或缓存状态混比。
- 提升某类控制吞吐，却让正常写、其他 root 或恢复任务饥饿。
- 成功 durable 回执在重启后丢失、重复发放执行权、终态倒退、未知外部结果被自动重放。
- 提交新的多连接存储却没有 after-commit 通知、lease/权限/删除竞态和迁移/回退证据。
- 把方法级测试通过等同于完整用户链路已达 SLO，或将未实际运行的测试写成已通过。

第一轮结束只表示基线、测量入口和迁移契约具备可接续证据；整体性能优化需要后续实现和同负载复测。
