---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-31
---

# 上下文管理

本文定义主模型上下文的事实来源、组装、计量和压缩。Trace、Exact Archive、历史检索与分叉见[Conversation Trace 与 Exact Archive](./conversation-trace-and-archive.md)；Provider wire 协议见[Agent Runtime 与模型 Provider](./agent-runtime-and-providers.md)；FileChange 的 Observation/投影边界见[FileChange 子系统](../subsystems/file-change.md)。

## 职责边界

上下文管理回答三个问题：

1. 下一次模型请求应看到哪些内容；
2. 这些内容占用多少上下文容量；
3. 超过安全容量时，哪个完整前缀可以被摘要替换。

它不负责保存 Provider 隐藏 reasoning、不把 Renderer timeline 当作历史、不决定文件或 Tool 权限，也不把 Exact Archive 原文无条件塞回主模型。

## 权威事实与派生视图

SQLite 中的 `messages`、终态 `ConversationTurnTrace`、当前 compaction head 及当前 Run 已确认状态组成逻辑日志。所有消费者从这些事实派生：

```text
messages + terminal traces + active compaction head
                         |
                         v
                  ContextAssembler
                         |
                         v
                    ContextFrame
      +------------------+------------------+
      |                  |                  |
      v                  v                  v
 capacity report    model request     context snapshot/UI
      |
      v
 compaction planner -> summary generation -> atomic commit
```

`agent_run_json`、Renderer 事件和 timeline 只用于展示，不得反向重建模型历史。终态 parent message 中冻结的 `collaborationTimelineActivities` 也只是展示 snapshot；final-response stream 开始后发生的子 Agent 活动仍留在 Agent Center/event log，但不会被补入已提交回复或模型上下文。Provider payload 必须从 Provider-neutral `LlmMessage` 生成；这适用于 OpenAI-compatible、Anthropic-compatible 和 DeepSeek，而不是只适用于某两个 Provider。

## 逻辑日志顺序

一个已结算 Turn 按下列逻辑顺序展开：

```text
user message
assistant public narration
Tool Call + Tool Result      # 一个不可拆分闭环
...
assistant final message
trace terminal record
```

消息位置和 Trace sequence 决定顺序，不能只按墙钟时间拼接。运行中的 narration 与已闭合 Tool Result 可先持久化；活动 Trace 只允许在尾部暂存一个用于审批/恢复的 open Tool Call，该调用不进入已闭合 model-context 前缀或压缩边界。最终助手消息和 Trace 终态在同一持久边界结算。

pending assistant message 的持久正文为空；“正在思考”等 UI placeholder 不属于逻辑日志。FileChange 的 `apply_patch` durable Tool Call 只保存 body-free operation 与 content/edit digest，不保存完整 content、old/new text 或 Observation ID；精确执行材料属于 Host 私有 action audit/Staged store。成功写后的 successor `fileChangeTarget` 只用于同 Run 的下一次模型 continuation 和私有 checkpoint，不进入普通 Event/Trace/Archive。

用户消息的持久正文保持原样。组装时后端可加入确定性的 `<backend_conversation_timing>` 元数据，记录该用户消息和相邻上一条 assistant 的时间；时间来自 SQLite 毫秒时间戳，并按本机时区渲染为带偏移的 RFC 3339。不要修改 assistant 历史正文来承载内部时间标记。

## ContextFrame 分类

同一计量器对完整帧分组，避免容量判断、UI 和实际发送使用不同口径：

- `fixed`：系统提示、Tool 定义和 Provider 固定开销；
- `durable`：当前摘要及其游标之后的已提交消息/Trace；
- `run_transient`：当前 Run 已产生、但尚未被主模型成功观察和提升的内容；
- `request_only`：Todo、文件事务提示、当前输入附件说明等只属于本次请求的材料；
- `reserved_output`：为模型输出保留的空间。

当前 Run 的 Tool Result 可以先落库，但在包含它的模型请求成功前仍属于 `run_transient`，不能被压缩覆盖。请求成功后，已观察前缀才能提升为 durable baseline。

## 组装规则

主请求的逻辑内容为：

```text
后端 system prompt
+ active semantic summary（若存在）
+ coveredThrough 之后的原始消息和闭合 Trace
+ 当前 Run transient
+ request-only 内容
```

后端 `ContinuityIndexV2` 用于定位与审计，默认不作为主模型消息发送。附件、Skill Resource、Artifact 与 Browser download 必须先通过统一 locator 和各自授权解析，模型字符串本身不是文件系统权限。Host 可以从 `agent_nodes` 推导同一 Agent task tree 的 exact root scope，使父/子/兄弟复用 durable attachment/artifact/download；模型、Prompt 或 Renderer 不能自报 root identity，未知 scheme-like/`@namespace` locator 必须 fail closed。

Automation HumanRoot Turn 还会追加 `automation_execution` 来源的 retained、Run-scoped system item，包含
Host 从持久 Run 绑定构造的任务/Run identity、计划时间、上次运行时间和 trigger kind。它不修改用户
Prompt，也不参与可复用 Conversation configuration revision；Approval/Checkpoint 恢复保留同一上下文。
普通 Turn 不含该 item。完整调度边界见
[Scheduled Automation](../subsystems/scheduled-automations.md)。

组装器应保持确定性：相同持久日志、active head、权限/工具集版本和请求输入应产生相同的逻辑帧。增量缓存只是优化；删除、重写、回退、分叉或摘要变化后，全量重建必须得到相同结果。

## 容量判断

`ContextCapacityDetector` 使用模型、API style、工具定义和 `ContextBudgetReport` 判断请求是否可发送。安全边界必须同时考虑：

- Provider/model context window；
- 当前请求估算输入；
- 为输出保留的预算；
- estimator 误差的 safety margin；
- Provider continuation 等非普通文本投影。

估算与实际 Provider usage 是两个不同事实：前者用于发送前防溢出，后者用于发送后计费和诊断。不能用返回 usage 回写历史内容，也不能因为一次估算偏差改变已经发送的请求。

## 稳定压缩游标

`ContextJournalCursor` 只能落在完整逻辑项之后：

- 完整 user/assistant message；
- public narration；
- 已闭合 Tool Result。

游标不得位于 Tool Call 与 Tool Result 之间，也不能覆盖 `run_transient`。规划器选择的是可被单个替换块替代的旧前缀，而不是简单删除最老若干 token。

## 摘要与 Continuity Index

`ContextCompactionSummary` 是不可变派生记录，包含上一摘要、稳定游标、源 revision、语义摘要、确定性 Continuity Index、计量诊断、模型和时间。active head 单独保存，因此提交失败不会破坏旧摘要。

摘要模型只读取：

```text
previousSummary + 新选中的原始日志前缀
```

输入历史按不可信数据处理。用户要求、助手主张、Tool/审批/终态证据必须区分；没有成功 Tool 证据的自述不得摘要成“已完成”。模型只生成 Markdown 语义正文，游标、revision、Continuity、token 统计和 active head 全由后端计算。

语义摘要应使用稳定的小节：

- Objective and constraints
- Confirmed state and decisions
- Completed work and artifacts
- Failures, approvals, and cautions
- Open work and next action

输出为空、被截断、夹带工具调用或未来替换块没有严格小于原前缀时，不得提交。

`ContinuityIndexV2` 只保存固定配额的 Message/Trace/Archive 引用和分类计数，不复制正文。当前硬上限为 1,500 tokens，常规目标不超过 800 tokens；它不进入主模型的日常 token 账单。引用必须属于同一 conversation 且仍存在。

## 压缩状态机

```text
capacity exceeded
  -> plan safe prefix
  -> receipt: planned
  -> prepare authoritative prefix + sourceRevision
  -> receipt: generating
  -> generate summary outside SQLite write transaction
  -> receipt: committing
  -> transaction re-reads prefix and checks head/revision/cursor
      -> commit observation + immutable summary + lineage + head + receipt
      -> or reject stale draft
  -> rebuild ContextFrame from SQLite
  -> remeasure and either send or compact again
```

摘要生成期间允许原始日志继续变化，但 commit 必须在事务内重新验证。游标之后追加不使摘要失效；覆盖范围内的编辑、删除或回退会改变 `sourceRevision`，使派生摘要失效或回到有效祖先。每个主请求有有限压缩尝试，无法获得实际缩小时返回明确容量错误。

## ModelRequestObservation

每次实际 Provider 请求在发送边界创建版本化观测：purpose、各 ContextFrame 分类估算、estimator identity/version、context revision、Provider usage、finish reason/有界错误及 billable request count。

这里的 ModelRequestObservation 是请求计量/诊断记录，不是 `read_file` 产生的 File Observation。它不保存 prompt、消息正文、Tool Result、网页/文件正文或 API key。普通请求的观测持久化失败不能重放已经完成的模型请求；压缩成功时 observation 与 summary/head/receipt 同事务提交。

## 分叉、删除与回退对上下文的影响

- 普通 UI 分叉和子 Agent snapshot 都复制已选择的完整终态 Turn，而不是共享原 conversation 的活动视图。
- `fork_turns=all` 可包含可见摘要链；最近 N 个 Turn 只复制选中的完整终态 Turn，不继承更老摘要；`none` 不复制历史。
- 运行中尾部、Usage、普通请求 Observation、Command Session 运行态、Checkpoint、draft 和可变 World State 不进入历史快照。
- 删除/回退使依赖被删除前缀的摘要失效；分叉会重写 Message/Trace/Archive 引用。

具体复制边界和 Exact Archive 规则见[Conversation Trace 与 Exact Archive](./conversation-trace-and-archive.md)。

## 不变量

1. `ContextAssembler` 是模型历史的唯一结构化入口。
2. ContextFrame 计量同时服务容量判断、压缩和 UI，不维护第二套 token 口径。
3. 压缩只替换模型视图，不删除原始消息、Trace 或 Exact Archive。
4. Tool Call/Tool Result 闭环不可被压缩边界拆开。
5. 模型无权选择 coveredThrough、sourceRevision、Continuity ref 或 active head。
6. `run_transient` 在主模型成功观察前不可压缩。
7. Provider 私有 continuation 不得伪装成公开上下文文本。
8. Automation execution context 只能由 Host 从持久 Run 构造，Renderer 或 Prompt 不能声明该身份。
9. pending assistant placeholder 与 collaboration timeline 都是展示状态，不得进入 Provider-neutral 历史。
10. FileChange call body/Observation authority 不进入普通 durable Trace；successor 只能由 commit 后验证生成并保留在模型/私有 checkpoint 边界。
11. Agent-tree 资源共享 scope 必须由 Host 从持久节点解析，locator 文本不构成授权。

## 代码真源

- 组装与帧：`crates/core/src/context/`
- Runtime 容量入口：`crates/core/src/runtime.rs`
- 容量与 UI snapshot：`crates/core-server/src/application/agent/context_window.rs`
- 压缩：`crates/core/src/runtime/context_compaction.rs`、`crates/core/src/runtime/context_compaction_model.rs`、`crates/core-server/src/application/agent/context_compaction.rs`
- 摘要存储：`crates/core/src/storage/context_compaction_repository.rs`
- Observation：`crates/core/src/model_request_observation.rs`、`storage/model_request_observation_repository.rs`
- 逻辑 Trace：`crates/core/src/conversation_trace.rs`
- Automation system context：`crates/core/src/protocol.rs`、`crates/core/src/runtime/preparation.rs`
- FileChange context projection：`crates/core/src/tools/apply_patch.rs`、`crates/core/src/file_change_support.rs`、`crates/core/src/runtime/checkpoint.rs`
- Tree resource scope：`crates/core/src/storage/agent_tree_resource_scope.rs`、`crates/core/src/resource_locator.rs`

## 测试

- `crates/core/src/context/frame/tests.rs`
- `crates/core/src/runtime/tests/conversation_context.rs`
- `crates/core/src/runtime/tests/compaction_and_tool_flow.rs`
- `crates/core/src/storage/context_compaction_repository/tests.rs`
- `crates/core-server/src/application/agent/tests/context_runtime.rs`
- `crates/core-server/src/application/agent/tests/context_history.rs`
- `crates/core-server/src/application/agent/context_compaction.rs` 内单元测试

## 变更检查表

- [ ] 新上下文来源被明确分类为 fixed、durable、run-transient 或 request-only。
- [ ] 容量检测、实际 request builder、Observation 和 UI 使用同一计量结果。
- [ ] 新日志项定义安全压缩边界，并覆盖 Tool Call/Tool Result 原子性。
- [ ] FileChange call body、Observation 与 successor 在 Trace/model/private checkpoint 之间保持脱权投影。
- [ ] 新虚拟资源由统一 locator 分类，并在组装前用 Host-resolved Conversation/Project/tree scope 授权。
- [ ] 摘要 schema、source revision、lineage、fork/rewrite/delete 行为均有迁移或失败策略。
- [ ] Provider 新增的私有内容由 Provider policy 计量，不泄漏进普通消息。
- [ ] 压缩失败、过期提交、取消和重启测试不改变旧 active head。
- [ ] 更新 Trace/Archive 文档和上下文契约测试。

## 当前限制

- token estimator 是发送前估算，不能保证与所有兼容 Provider 的计费完全一致。
- 压缩依赖当前模型生成有效且更小的摘要；最多有限尝试，不承诺任何输入都可压缩。
- ContextFrame 的增量缓存只在 revision 连续时有效，复杂重写会退回全量重建。
- Continuity Index 是有限引用集合，不是完整目录；精确内容必须通过历史工具打开。
- 正在执行且未提交的流式模型片段不会成为可恢复的长期上下文。
- final-response stream 开始后的协作活动不会回填已冻结的 parent message timeline；完整后续状态需从 Agent Center/event log 查询。
