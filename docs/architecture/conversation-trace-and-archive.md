---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-23
---

# Conversation Trace 与 Exact Archive

本文定义 Agent 活动如何形成可重建、可审计的长期历史，以及模型、Renderer、Trace 和 Exact Archive 为何必须使用不同投影。

## 职责边界

本子系统负责记录已经发生的 Provider-neutral 活动、保存模型当时实际观察到的安全投影，并为大文本提供按需精确读取。它不保存隐藏 reasoning，不把 Renderer timeline 作为事实源，不决定 Tool 权限，也不使用当前 Tool 代码重新计算旧历史。

## 三类持久事实

| 事实                    | 用途                                                                    | 是否保存正文 | 是否可作为模型历史              |
| ----------------------- | ----------------------------------------------------------------------- | ------------ | ------------------------------- |
| `messages`              | 用户消息、助手最终回复及消息状态                                        | 是           | 是                              |
| `ConversationTurnTrace` | narration、Tool Call/Tool Result、审批/Run 终态的 Provider-neutral 序列 | 有界安全投影 | 是，经 model-context projection |
| Exact History Archive   | Tool 实际捕获到的安全原文与完整性元数据                                 | 分块无损保存 | 仅按需分页打开                  |

此外，`conversation_model_context_*` 保存模型实际观察过的安全投影；它与审计 Trace 并行，不能用当前工具实现重新计算旧结果。`ModelRequestObservation` 只保存计量和请求终态，不是内容仓库。

## Trace 生命周期

```text
Turn created
  -> trace in_progress
  -> append public narration
  -> append closed Tool Call/Tool Result exchanges
  -> may stage one final open Tool Call while in_progress
  -> optional approval/recovery records
  -> terminal: completed | failed | cancelled
```

当前 Trace schema version 为 `CONVERSATION_TURN_TRACE_SCHEMA_VERSION = 4`。公开 narration 一旦发出即可追加。长期历史中的 Tool Call 与 Tool Result 必须闭环；为支持审批和崩溃恢复，`in_progress` Trace 可在尾部持久化一个尚未结算的 open Tool Call，并由 Runtime/Checkpoint/pending action 保存恢复权威。该 open call 不进入已闭合 model-context 前缀，Trace 进入终态前必须由权威或合成 Tool Result 关闭。已持久化 sequence 是 append-only，不能重排、缩短或重新打开。

最终 assistant message、Trace terminal、model-context projection、Usage 和 UI 终态由 Core Server 在同一结算边界提交。进程崩溃后，startup reconciliation 只在已持久证据明确证明 Run 已取消时结算为 `cancelled`；其他孤立的 `in_progress` Trace 结算为 `failed`。Trace 没有 `interrupted` 终态，也不从 Renderer 看似完成的状态推断成功。

Automation Run 在原子 HumanRoot admission 时绑定 `agent_run_id`、Conversation 和 user/assistant message。
Automation observer 以持久 Trace terminal 和 pending action 判断 `running`、`waiting_for_approval` 与终态，
不能从 assistant 文案或进程内 worker 状态猜测成功。Automation 专用 `automation_report` 另写入 Run-scoped
结构化结果，最终通知 policy 使用其 `no_change`/`important_update`/`completed`/`unknown` 投影；它不改变
Trace 的权威终态。详见 [Scheduled Automation](../subsystems/scheduled-automations.md)。

## 消费者投影

同一个 canonical Tool Result 可能包含模型行动信息、UI 展示字段、审计身份和恢复凭据。每个 Tool 通过 `ToolRegistry` 明确定义：

- Model projection：下一步推理所需的最小语义；
- Event projection：Renderer 可展示且无私有路径/秘密的数据；
- Durable Trace projection：长期审计所需的有界数据；
- Exact Archive projection：安全清洗后、不做普通文本长度截断的捕获内容；
- Checkpoint projection：暂停恢复所需且可持久化的安全状态；
- Runtime Extension projection：仅给同一 Run 中的扩展消费者。

投影是单向的，不能用 Event 或 Trace 重建 canonical result，也不能用 Renderer payload 恢复审批。详细字段边界见[Tool Result 消费者矩阵](../subsystems/tool-result-consumer-matrix.md)。

## Exact Archive 写入

正文型结果在 Durable Trace 限长和中央 Model Gate 之前进入共享 Exact Capture：

1. Tool/Provider 捕获数据并声明来源完整性、原始/捕获/遗漏量和停止原因。
2. 统一安全投影移除二进制、data URL、私有路径、secret 等不应留存内容。
3. 计算 SHA-256、字符/字节计数和内容类型。
4. UTF-8 正文以独立 zstd chunk 写入 `conversation_history_blobs` 与 `conversation_history_blob_chunks`。
5. Trace result 仅保存 `archiveRef`、hash、归档量和各阶段截断状态。

当前正文/进程输出共享的单次 Exact Capture 上限为 64 MiB。命令、Git、Office 和 Skill Script 的 stdout/stderr 在执行期间写入 spool；128 KiB 预览不是 archive 完整性的边界。

`archivedCompletely=true` 只表示安全清洗后、实际到达本进程的内容被完整归档。它不能证明上游网页、Provider 或进程没有在更早阶段丢失数据。`truncatedAtSource=true` 的遗漏无法由 Archive 恢复。

## 历史读取与递归防护

`conversation_history` 支持目录、范围、around 和 archive open。读取必须：

- 在 SQL 边界绑定当前 conversation；
- 使用不透明引用和稳定分页，不能接受任意数据库 ID 作为授权；
- 对字符或 UTF-8 字节边界安全分页；
- 返回 hash、范围、完整性和导航信息；
- 继续经过中央 Model Result Gate。

历史 Tool 取回的 Archive 正文不再次写入新的 Exact Archive。其 Durable Trace 只记录 query/ref、页范围、hash、状态和返回量，避免“读取历史”无限复制同一正文。当前 Run 中读取到的页面仍可供模型使用，直到被正常 compaction 覆盖。

SQLite FTS5 只是可重建检索索引，不是第二份权威日志。命中结果必须回到 conversation/message/trace/archive 归属检查后才能返回。

## Continuity Index

Compaction 的语义摘要负责保留任务含义；`ContinuityIndexV2` 只提供有限的精确入口。它由后端确定性生成，可引用同一 conversation 的 Message、Trace Item 或 Archive Blob，按任务证据、未解决失败、审批、重要决定和最近记录使用固定配额，去重后最多 28 个引用。

Index 不复制正文、普通 narration 或每个成功 Tool 的 operation/outcome。V1 可读取；后续成功压缩会生成 V2。提交和加载时验证引用归属与存在性。

## Rewrite、回退、分叉与删除

- 覆盖范围内编辑/删除改变 source revision，使依赖它的 compaction 派生状态失效。
- Turn rewrite 保留明确的 rewrite record，并在事务内更新消息、Trace/model context、Archive 引用及派生状态；不能就地伪造旧 sequence。
- 分叉复制选中边界内的终态消息、Trace、model-context、摘要链、附件和相关 Artifact grant。Archive chunk 可复用内容，但目标必须获得新归属/ref，并重写目标 Trace 引用。
- Usage、普通请求 Observation、运行中 Checkpoint、Command Session 活动状态和 mutable world state 不作为历史内容复制。
- 会话删除通过外键/服务事务清理内容和授权；审计表是否保留由其数据生命周期定义，不能仅依赖级联猜测。
- 删除 Automation 绑定的 Conversation/message/project 前，服务事务先 terminalize 相关活动 Run 并请求
  Agent 取消，再删除 Trace；不得让外键级联先抹去恢复证据。

协作 `fork_turns=all|N|none` 只选择完整 settled Turn。被选择的 settled assistant 缺少必需 Durable Trace 时应失败，而不是创建无法验证的历史快照。

## 不变量

1. Trace 是 Provider-neutral、append-only、无隐藏 reasoning 的活动日志。
2. Tool Call/Tool Result 是终态长期历史中的原子闭环；活动 Trace 仅允许一个可恢复的尾部 open call。
3. Model、Event、Trace、Archive、Checkpoint 投影互不替代。
4. Exact Archive 在普通 Trace/Model 限长前写入，但仍受安全清洗和 capture 硬限。
5. Source truncation、semantic pagination、consumer truncation 和 model truncation 必须分别记录。
6. Archive/FTS/history ref 不能绕过 conversation、project、attachment 或 Artifact 授权。
7. 分叉后的历史不依赖源会话继续存在。

## 代码真源

- Trace 模型与规范化：`crates/core/src/conversation_trace.rs`、`conversation_trace/`
- Model-context 类型与投影：`crates/core/src/conversation_trace.rs`、`crates/core/src/context/trace_renderer.rs`
- Model-context 存储：`crates/core/src/storage/conversation_model_context_repository.rs`
- Trace 存储：`crates/core/src/storage/conversation_trace_repository.rs`
- Archive：`crates/core/src/storage/conversation_history_archive_repository.rs`
- 历史查询：`crates/core/src/storage/conversation_history_repository.rs`、`conversation_history_open.rs`、`crates/core/src/tools/conversation_history.rs`
- Rewrite/fork：`storage/conversation_turn_rewrite_repository.rs`、`conversation_fork_repository.rs`
- 终态与修复：`crates/core/src/storage/service/messages.rs`、`trace_reconciliation.rs`
- Tool 投影入口：`crates/core/src/tools/mod.rs`
- Automation Trace observer/admission：`crates/core-server/src/application/automation/scheduler.rs`、
  `crates/core-server/src/application/agent/automation_turn.rs`

## 测试

- `crates/core/src/conversation_trace/tests.rs`
- `crates/core/src/storage/conversation_history_archive_repository.rs` 内测试
- `crates/core/src/storage/conversation_fork_repository/tests.rs`
- `crates/core/src/storage/service/tests/trace_reconciliation.rs`
- `crates/core/src/storage/service/tests/turn_rewrites.rs`
- `crates/core-server/src/application/agent/tests/context_history.rs`
- `crates/core/src/runtime/tests/trace_and_projection.rs`
- `crates/core/tests/fixtures/tool_result_projection_contract_v1.json`
- `crates/core-server/src/application/automation/scheduler/tests.rs`
- `crates/core-server/src/application/agent/tests/automation_turn.rs`

## 变更检查表

- [ ] 新 Trace item 定义 sequence、终态、model-context 和 fork/rewrite 行为。
- [ ] 新 Tool 显式实现各消费者投影，并说明是否进入 Exact Archive。
- [ ] 新正文捕获路径使用共享 capture/spool，声明 source completeness。
- [ ] history ref 在读取、分叉、删除、重写和会话隔离测试中均被校验。
- [ ] startup reconciliation 对新 `in_progress` 状态有保守结算规则。
- [ ] Automation Run 绑定、Approval 恢复与资源删除是否仍由持久 Trace 驱动且原子收口。
- [ ] FTS/schema 变化保持索引可重建，权威内容不依赖索引。
- [ ] 防止 `conversation_history` 结果递归归档。

## 当前限制

- 单次 Exact Capture 为 64 MiB；更早的 Provider/格式解析安全限仍可能造成来源截断。
- Archive 只保证安全捕获内容的精确性，不保存二进制 payload、隐藏 reasoning 或 secret。
- FTS 查询不是语义向量检索，且旧/损坏索引需要通过启动或维护流程重建。
- 运行中尚未提交的模型流、Tool 调用和 narration 在进程崩溃时可能只剩活动 Trace/Checkpoint 已覆盖部分。
- 历史分叉复制的是选中时刻的不可变快照，不会与源会话继续同步。
