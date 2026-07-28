# 上下文管理架构

本文说明 MyCopilot 当前的上下文事实来源、模型请求组装、运行中 Agent 轨迹、统一计量、自动压缩和前端上下文圆环。

Tool 结果的 Model、Renderer、Runtime Extension、Durable Trace、Exact Archive 与审批
Checkpoint 消费边界，见 [Tool Result 消费者矩阵与投影契约](tool-result-consumer-matrix.md)；
统一 10K、来源安全限和恢复语义见
[Tool Result 上限、投影与恢复契约](tool-result-limits.md)。

## 核心原则

系统只维护一份逻辑上的原始上下文日志：

```text
用户/助手消息 + 助手公开过程 + 工具调用 + 工具结果 + 运行终态
```

所有消费者都从这份日志派生自己的视图：

- 主模型看到“系统提示词 + 当前摘要 + 摘要游标后的原始日志 + 本次请求临时内容”。
- 压缩器读取“上一版摘要 + 游标之后待压缩的原始日志前缀”。
- 容量保护和上下文圆环读取同一个 `ContextFrame` 分类计量结果。
- 精确旧记录查询读取同一份 SQLite 消息、trace 与其无损 Exact History Archive，不从摘要或前端事件反推。
- 前端 timeline 使用展示事件，但展示事件不是模型上下文的事实来源。

压缩不会删除原始消息或 Agent 轨迹，只会生成一版摘要并向后移动一个稳定游标。删除消息、回退或删除会话才会改变原始日志。

Exact History Archive 是原始工具结果的无损安全投影，不是模型上下文镜像；SQLite FTS5 是可重建的检索索引，不是另一份权威日志。系统不实现 run-overlay 专用压缩路径。`ModelRequestObservation` 也不属于内容仓库：它只记录发送边界的分类 token 估算、provider usage 和请求终态，不保存 prompt、消息正文、工具结果或密钥。

## 总体数据流

```text
SQLite messages + ConversationTurnTrace
                    |
                    v
         logical context journal
                    |
          +---------+-------------------+
          |                             |
          v                             v
summary + ContinuityIndexV2       conversation_history
          |                     (FTS/read/around/range)
          v
raw suffix after cursor
          |
                    v
             ContextAssembler
                    v
      AgentConversationContextState
       (classified measured baseline)
          |                   |
          v                   v
 Agent runtime request   context window snapshot
          |                   |
          v                   v
 capacity + compaction      frontend circle
          |
        v
 OpenAI / Anthropic payload
        |
        v
 ModelRequestObservation
        |
        v
 compaction receipt
```

`ContextAssembler` 是模型上下文的唯一结构化组装入口。OpenAI 和 Anthropic payload 都由组装后的 provider-neutral `LlmMessage` 生成，不读取前端 `agent_run_json`，也不从 timeline 反推历史。

## 逻辑上下文日志

SQLite 中的物理数据由两部分组成：

- `messages`：用户消息、助手最终回复、状态和 `created_at`。
- `ConversationTurnTrace`：助手公开的中间说明、工具调用、工具结果和运行终态。

它们按消息位置和 trace sequence 展开为一条逻辑日志。一个正常 Agent turn 的顺序是：

```text
user message
assistant narration
tool call
tool result
assistant narration
tool call
tool result
assistant final reply
terminal record
```

下一条用户消息排在这些记录之后。

消息时间使用 SQLite `messages.created_at` 的 Unix 毫秒值。组装请求时按本机时区渲染为带 UTC 偏移的 RFC 3339 时间，但不修改数据库正文，也不装饰 assistant 历史正文。每条 user 消息开头由后端加入 `<backend_conversation_timing>` 元数据块：保存该 user 消息时间，并在存在时同时保存紧邻上一条 assistant 消息时间。这样模型仍能判断对话时序，同时不会从 assistant 历史样本中学到并复述内部标记。

完整上下文重建与会话级增量缓存共用同一个时间状态机。元数据是确定性的 retained 内容，参与统一 token 计量；压缩、删除或回退导致完整重建时也遵循同一渲染规则。

## ConversationTurnTrace

`ConversationTurnTrace` 是后端生成、provider-neutral、append-only 的 Agent 活动日志，不保存隐藏 reasoning。

Trace 保存 provider-neutral 的活动与审计投影；并行的 model-context projection 保存主模型实际观察过的精简结果：

- narration 保存公开过程说明；
- tool call 保存模型生成的完整文本参数；
- tool result 的 Durable Trace 保存有界审计负载，model-context projection 保存当时模型收到的语义投影；
- Base64、data URL 等二进制内容在统一边界移除；
- 不按工具类型制作另一份长期摘要，不做语义去重，不因长度静默丢弃文本。

同一次工具执行只产生一份规范化的模型语义结果。当前 tool loop、审批恢复和后续精确 model-context 重建使用同一份结果；前端事件、Durable Trace 和 Exact Archive 独立投影，不能反向成为模型上下文。

运行中提交规则：

- narration 一旦成为公开输出即可追加；
- tool call 与 tool result 是不可拆分闭环，未闭合调用只保存在运行检查点；
- 闭环完成后立即追加 SQLite，圆环随之更新；
- 已提交 trace 前缀不能重写、缩短或重新打开；
- 最终助手消息、trace 终态和 UI 终态在同一事务提交。

相关表：

- `conversation_turn_traces`
- `conversation_turn_trace_items`

删除助手消息或会话时由外键级联删除 trace。

## 活跃模型视图

主模型每次请求看到：

```text
后端系统提示词
+ 当前语义摘要（如果存在；Continuity Index 只保留在后端）
+ 摘要游标之后的原始消息和 trace
+ 当前 run 尚未提升到长期基线的内容
+ Todo、文件事务提示等 request-only 内容
```

当前 run 的工具结果会立即落库，但在主模型成功收到它之前仍留在 `run_transient` overlay，不能参与压缩。一次 provider 请求成功返回后，该请求中包含的 trace 前缀才被提升为可压缩的 durable 基线。

这不是两套日志。SQLite 原始日志只有一份；durable baseline 和 run overlay 只是同一日志在“模型已见游标”两侧的两个投影。

## 稳定压缩游标

`ContextJournalCursor` 表示摘要已经覆盖到逻辑日志的哪个位置：

- `Message { message_id }`：覆盖完整用户消息或助手最终消息。
- `TraceItem { assistant_message_id, sequence }`：覆盖到某条 narration 或闭合 tool result。

压缩边界可以落在：

- 完整消息之后；
- narration 之后；
- tool result 之后。

压缩边界不能落在 tool call 与 tool result 之间。历史 trace 渲染时，同一个工具闭环共享 result sequence 作为原子来源，因此规划器无法拆开它们。

## ContextCompactionSummary

`ContextCompactionSummary` 是不可变、版本化的派生记录，包含：

- 摘要 ID、会话 ID 和 schema version；
- 上一版摘要 ID；
- `coveredThrough` 稳定日志游标；
- 完整原始前缀的 `sourceRevision`；
- 模型生成的语义摘要；
- 后端确定性生成的 `ContinuityIndexV2`（只用于后端定位和审计）；
- `summaryInputTokens`、`continuityInputTokens`、`uncoveredTailInputTokens` 和 `replacementInputTokens` 四项 token 诊断值；
- 生成模型和创建时间。

SQLite 使用三张职责单一的摘要状态表：

- `context_compaction_summaries`：不可变摘要版本。
- `context_compaction_summary_lineage`：记录摘要由哪条 assistant 回复引入，以及复制来源。
- `conversation_context_compaction_heads`：会话当前生效的摘要。

原始日志本身已经表达覆盖范围，因此不再维护 `context_compaction_summary_sources` 之类的消息 ID 镜像表。

摘要的因果归属不是从时间戳猜测出来的。正常压缩提交时，摘要、归属、active head、请求观测和 receipt 在同一事务内提交；删除或回退引入该摘要的 assistant 回复时，归属外键会使这版摘要及其后继 active 状态失效。`source_conversation_id` 和 `source_summary_id` 只记录任务分叉来源，不构成对原任务的生命周期依赖。

多次压缩时，摘要模型读取：

```text
上一版摘要 + 上一游标之后、本次新覆盖的原始日志项
```

它不会重新读取已经被上一版摘要覆盖的全部原文。原始数据仍保留在 SQLite，供审计、回退和派生状态失效后校验。

## 在新任务中继续

“在新任务中继续”创建的是所选 assistant 回复处的独立历史快照，不是指向原任务的视图。分叉操作由后端以幂等 `requestId` 编排；除附件文件的暂存与原子改名外，所有 SQLite 数据在一个事务中提交。

快照包含：

- 从原任务开头到所选 assistant 回复的 user/assistant 消息及原始时间；
- 这些回复对应的 `ConversationTurnTrace`、终态和文件编辑预览草稿；
- 消息附件的独立文件副本和新的附件 ID；
- 截止该回复已经引入的完整摘要祖先链，而不只是当时的 active head；
- 相同项目、模型和权限选择，并使用全新的消息、run、trace、draft 和 summary ID。

快照不包含原任务在所选回复之后的消息，也不复制 provider usage、`ModelRequestObservation`、压缩 receipt、审批审计或运行中检查点。历史消息里的七项 usage 字段统一归零，因为新任务没有再次产生这些计费事实。摘要的结构化 token 诊断值仍随摘要复制，它们描述摘要本身，不是新任务的模型账单。

分叉后的任务可以继续分叉。每一层都只读取自己的消息、trace 和摘要 lineage；删除原任务后，后代任务的附件、文件预览与摘要链仍然可用。原任务 ID 只作为不可解析的 provenance 文本保留，不能通过外键或运行时查询影响新任务。

### Exact History Archive

工具结果存在三种相互独立的投影：

- Runtime Model Projection：当前工具闭环交给模型的结果；
- Durable Trace Projection：经过工具级和集中式限长后进入长期 trace 的审计投影；
- Exact History Archive Projection：经过必要的安全/二进制清洗、但不做文本长度截断的精确历史投影。

Exact History Archive 在 Durable Trace 限长之前提交。`conversation_history_blobs` 保存会话、assistant 消息、trace sequence、call ID、tool、内容类型、原始字节/字符数、SHA-256 和截断语义；`conversation_history_blob_chunks` 使用独立 zstd UTF-8 分块保存正文。Trace ToolResult 只保留 `archiveRef`、`contentHash`、`archivedBytes` 以及 source/model/history/archive 四个不同阶段的截断状态。

正文型结果使用共享 64 MiB Exact Capture；进程 stdout/stderr 共享同一捕获配额并流式写入 spool。Canonical Result 归档后，所有模型结果统一经过固定 10K Gate，超限时由 `historyOpen`、工具 cursor 或 `continueWith` 恢复。

`truncatedAtSource=true` 表示工具在生成结果时已经只返回了部分外部资源；Archive 可以精确恢复“当时实际返回的结果”，但不能恢复工具从未取得的剩余资源。`archivedCompletely=true` 表示安全清洗后的工具结果已经完整写入 Archive，不证明未声明完整性的上游 Provider 返回了完整外部资源。历史读取支持字符或 UTF-8 字节分页，始终在 SQL 边界校验当前 `conversationId`。

`conversation_history` 自身不归档取回的正文，其 Durable Trace Projection 只保留 query/ref、页范围、hash、状态和返回字符数，防止历史回忆再次复制到历史仓库或长期上下文。会话删除通过外键级联清理 Archive；会话分叉复制可见边界内的压缩块并重写目标 Trace 的 archive ref，因此删除原会话不会破坏分叉后的精确历史。

### Continuity Index V2

语义摘要负责保留任务语义；`ContinuityIndexV2` 只负责提供固定上限的精确历史入口。它由后端根据原始事件类型和终态生成，模型不能填写或修改：

```ts
interface ContinuityIndexV2 {
  schemaVersion: 2
  coveredThrough: JournalCursor
  taskEvidenceRefs: HistoryRef[]
  unresolvedFailureRefs: HistoryRef[]
  approvalRefs: HistoryRef[]
  importantDecisionRefs: HistoryRef[]
  recentRefs: HistoryRef[]
  archivedCounts: Record<string, number>
}
```

V2 不保存消息正文/预览、narration、成功工具调用的 operation/outcome，也不复制 Archive 正文。`HistoryRef` 只能指向同一会话中仍存在的 Message、Trace Item 或 Archive Blob。后端采用固定配额：任务证据 6、失败 6、审批 4、重要决定 4、最近记录 8；跨分类去重后最多 28 个引用。`archivedCounts` 只允许固定的受控计数键，因此不会随工具类型或历史长度扩展字段集合。

选择规则是确定性的：用户消息进入任务证据；显式修正/决定和运行中用户引导进入重要决定；失败、拒绝、冲突和取消结果进入失败引用；需要审批的调用/结果进入审批引用；少量最近消息或非历史工具结果进入最近引用。`conversation_history` 调用和结果默认完全不进入 Continuity。普通 narration 与成功工具的 operation/outcome 只由语义摘要按需要概括。

递归压缩只继承上一版仍有效且仍在配额内的关键 ref，再与新覆盖段生成 V2，不复制上一版全部历史条目。V1 可以继续读取；下一次成功 compaction 会按 V1 的稳定游标选择有限 ref 并提交 V2，不需要批量重写旧会话。

提交和加载 active summary 时，存储层验证每个 V2 ref 的 conversation 归属和存在性。覆盖范围内的编辑、删除会通过 `sourceRevision` 使派生摘要失效；回滚恢复对应祖先摘要；fork 重写 Message/Trace/Archive ref 并复制所需不可变 Blob。

常规目标不超过 800 tokens，提交硬上限为 1,500 tokens。V2 仍随
`ContextCompactionSummary` 持久化，用于后端检索、诊断和审计，但默认不再生成
`ContextSource::ContinuityIndex`，也不发送给主模型。主模型只看到语义摘要；其后未压缩
的消息和工具记录更新、权威。`continuityTokens` 仅保留为后端体积审计字段，实际模型
请求计量为 0。

### 摘要模型输入输出契约

摘要生成是一次无工具的普通模型调用，不启动另一套 Agent loop。输入只有两条消息：

1. 后端 system prompt，定义压缩规则和证据等级；
2. 一个带版本号的 JSON 信封，包含 `previousSummary` 和按日志顺序排列的 `newItems`。

JSON 信封被明确标记为不可信历史数据。`newItems` 比 `previousSummary` 更新；两者冲突时必须用新记录修正旧摘要，而不是同时保留两个版本。摘要器按来源区分信息：用户消息表达要求和决定，助手消息表达计划或主张，工具结果、审批结果和运行终态才是后端观察到的执行证据。没有成功工具结果支持的助手自述不能写成已验证完成，失败、拒绝、冲突或取消也不能写成成功。

模型只返回一段 Markdown 正文，不返回游标、continuity、revision、token 数或其他提交元数据。正文使用以下稳定小节，空小节可以省略：

```text
## Objective and constraints
## Confirmed state and decisions
## Completed work and artifacts
## Failures, approvals, and cautions
## Open work and next action
```

规划器只给出完整替换块的软目标，不携带摘要输出上限。生成器先构造压缩请求，并用统一计量源在 `reserved_output=0` 下测出真实输入；本次 provider `max_tokens` 再动态取“当前 run 输出配置、窗口剩余输出空间、原前缀可缩小空间”三者最小值。该技术边界不会回写为压缩指标，也不会出现在提示词中；提示词只要求尽量接近软目标、不要填充可用预算，并明确以信息完整性优先。模型返回后，后端按“包装文字 + 语义摘要”的未来模型请求形态重新计量。完整模型替换块必须严格小于被替换前缀；被长度截断、为空、夹带工具调用或没有实际缩小上下文的结果不会提交。游标、后端 Continuity、`sourceRevision` 和 active head 仍由后端绑定和复核，模型无权生成或修改这些字段。

## 原子压缩流程

```text
容量检测
  -> 纯规划器选择一个安全日志前缀
  -> 创建 planned ContextCompactionReceipt
  -> prepare 从 SQLite 读取前缀并计算 sourceRevision
  -> receipt 进入 generating
  -> 后端从该前缀生成固定配额 ContinuityIndexV2
  -> generate 在事务外调用当前 run 固定模型
  -> 生成 ModelRequestObservation，但成功路径暂不单独提交
  -> receipt 进入 committing
  -> commit 在写事务内重新读取同一前缀
  -> 校验 active head、游标和 sourceRevision
  -> 原子写入 observation、不可变摘要、active head 和 applied receipt
  -> 从 SQLite 权威日志重建基线
  -> 重新计量、重新规划，再决定是否发送主请求
```

摘要生成期间原始前缀或 active head 变化时，本次草稿不会提交，而是刷新权威基线并重新规划。摘要提交失败不会改变旧 head。

原始日志在摘要游标之后继续追加，不会让摘要失效；游标覆盖范围内的消息被编辑、删除或回退时，`sourceRevision` 校验失败，派生摘要被丢弃并回到原始日志。

### ModelRequestObservation

每次实际 provider 请求都在同一个发送边界建立一条版本化观测：

- `purpose` 区分主 Agent loop 与上下文压缩；
- 请求前估算直接取自本次容量判断使用的同一个 `ContextBudgetReport`；
- 保存 fixed、durable、run-transient、request-only 分类值和总估算；
- 保存 estimator identity、版本、增量/整帧计量模式和 context revision；
- provider 返回后保存原始 `AgentUsage`、标准化输入 token、finish reason 或有界错误；
- OpenAI cached input 已包含在 input tokens 内，不重复相加；
- Anthropic 按 input + cache read + cache creation 还原完整输入口径；
- 不保存请求 messages、tools、网页正文、文件正文、Base64 或 API token。

网络重试可能让一个逻辑请求的 usage 汇总多次计费尝试。观测保留该原始事实和 `billableRequestCount`，但验收报告不会把多次尝试的累计输入与单次发送前估算直接比较。

普通 Agent 请求的观测独立落库；写入诊断失败不会重放已返回的模型响应，避免重复工具副作用。压缩请求的成功观测由压缩提交事务统一写入。

### ContextCompactionReceipt

每次压缩尝试使用稳定 `operationId` 建立一条 receipt，状态机为：

```text
planned -> preparing -> generating -> committing -> applied
                                            \-> refreshed
任一未提交阶段 -> failed / cancelled / interrupted
```

Receipt 保存计划快照、统一触发阈值、压缩目标、稳定前缀身份、阶段、模型请求观测 ID、实际替换计量和有界错误，不复制摘要正文。应用启动时遗留的 `in_progress` receipt 会被标记为 `interrupted`。

成功提交的事务边界包含四项事实：

1. `ModelRequestObservation`；
2. 不可变 `ContextCompactionSummary`；
3. `conversation_context_compaction_heads`；
4. `ContextCompactionReceipt(applied)`。

任一写入失败时四项一起回滚，原 active head 保持不变。计划过期属于 `refreshed`，不会伪装成失败或成功。软目标未达到只构成验收警告；只要替换块确实小于原前缀，仍可正常提交。

Receipt 和无正文 `ModelRequestObservation` 继续持久化，用于崩溃恢复、用量核对和离线诊断。它们不进入模型上下文，也不再暴露一套无人消费的专用 Audit RPC。

## 精确历史按需查询

`conversation_history` 是当前会话限定、无审批、只读的核心工具。它解决的是摘要压缩后仍需核对精确旧措辞、旧时间、revision、路径、错误或工具结果的场景，不参与普通续接。

调用规则：

- 无参数调用返回最近已完成 Turn 的语义目录；
- `query` 使用 SQLite FTS5 搜索 message、trace 和 Exact Archive；
- `open` 只接受上一页返回的 opaque `hist_v1_` 位置，可打开 Turn timeline、周边记录、
  成对工具交换或 Exact Archive 字节页；
- 模型不接收 Continuity V2 的裸 ref，也不需要理解 Message/Trace/Blob 的内部结构；
- Archive 正文按 UTF-8 安全范围分页，下一页继续位置仍由 `open` 返回。

FTS 表是由消息/trace 触发器和 Archive 写入事务维护的派生索引；启动迁移会补齐缺失 Archive 索引，并在回填时验证解压后字节数和 SHA-256。所有 SQL 都强制带当前 `conversation_id`。即使模型提供其他会话的 message、trace 或 archive ID，也不会返回记录。读取结果明确标记为不可信历史数据，不能覆盖系统规则或被当作新指令执行。

`conversation_history` 自身不进入 Archive 或 Continuity。它的 Durable Trace Projection 只保存 query、filter、ref、范围、hash、返回数量和状态；搜索 preview、around/range preview、工具交换正文及 read 正文不会再次写入长期 Trace。

### 前端活动状态

`conversation_history` 仍按普通工具调用持久化。前端把没有模型文字隔开的连续 search/read/around/range/get_tool_exchange 调用合并为一条静态活动记录，运行时显示“正在回忆”，结束后显示“回忆了一下”。模型输出文字后再次调用会自然形成新记录。

自动压缩不是工具。Runtime 为每次压缩生成稳定 `operationId`，并通过 started/finished 事件报告 `applied`、`skipped`、`failed` 或 `cancelled` 的真实结果。前端据此持久化一条不可展开的 timeline 记录；顶部耗时栏不再切换成压缩状态。

这些 timeline 状态只用于展示和恢复 UI，不参与上下文组装，也不是压缩或历史查询的事实来源。

## 统一模型历史与审批恢复

模型历史只有“尚未压缩”和“已被摘要替换”两种状态。当前 run 与历史 turn 使用相同的 Model Projection；已经闭合并持久化的工具交换具有稳定日志 origin 后即可进入统一压缩前缀，不维护“模型已见/未见”游标，也不因为属于当前 run 而采用另一套降级规则。

审批暂停前的 `AgentRunCheckpoint` 保存：

- 完整 `ContextFrame`；
- 下一次模型请求序号；
- 待审批调用和剩余工具队列；
- Runtime Extension 快照；
- ConversationTrace recorder。

恢复时不会重放已经完成的副作用工具。尚未配对完成的 tool call 和审批 checkpoint 受到绝对保护，不参与压缩。

## 统一分类与计量

每个 `ContextItem` 都携带来源、scope、retention、稳定 origin 和可选工具原子分组。计量映射为四类：

| 分类          | 含义                                         | 压缩行为                             |
| ------------- | -------------------------------------------- | ------------------------------------ |
| fixed         | 系统提示词、工具定义、provider 协议开销      | 不压缩                               |
| durable       | 会话摘要、原始消息和已持久化历史             | 有稳定日志 origin 时进入统一前缀     |
| run_transient | 当前 run 的模型回复、工具协议和运行覆盖      | 闭环且有稳定日志 origin 时同样可压缩 |
| request_only  | Todo、文件事务提示等只服务本次请求的临时内容 | 不写入长期日志                       |

计量链只有一套：

1. `ContextTokenEstimator` 估算消息、工具定义、图片和协议开销。
2. `ContextFrame` 缓存每个 item 的估算并增量维护分类汇总。
3. `ContextCapacityDetector` 生成一个 `ContextBudgetReport`。
4. 真正发送 provider 请求时，将同一报告投影为无正文 `ModelRequestObservation`。

同一个报告派生：

- provider 请求容量判断；
- `ContextCompactionQuery`；
- 前端 `AgentContextWindowSnapshot`。

消费者不维护自己的 token 公式。

当前 estimator 是启发式实现，并已预留 estimator identity 和整帧复核接口，未来可以替换为模型专用 tokenizer。

### 工具文本输出预算

容量保护从同一个 `ContextCapacityDetector` 派生不可变的 `ContextTextBudget`，随运行时
上下文注入工具。它不是第二套 token 计算：文本估算和最终请求使用本次模型选中的同一个
`ContextTokenEstimator`。

生产运行时对每条最终模型 Tool Result 使用固定 10K token 产品上限。中央
`ModelToolResultGate` 覆盖静态 Tool、Runtime Extension、审批后 Host Tool 和恢复路径；
不提供每模型或每工具配置。支持分页的工具会在生成 cursor 前用同一 10K budget 测量
最终语义投影，确保中央 Gate 不会再次裁掉已经计入 cursor 的结果项。

`read_file` 等正文分页工具：

- 不再按文件类型设置 400 行默认值或 2,000 行硬上限；
- 不再因文件超过 512 KiB 直接失败；
- 未指定范围时尽量返回完整 UTF-8 文件；
- 超过额度时在完整行边界优先截断，单个超长行则在 UTF-8 字符边界截断；
- 返回 `nextStartByte`、`nextStartLine` 和 `nextStartColumn`，下一次读取可无损续接；
- 文件扫描和 revision 计算使用固定大小缓冲区，revision 仍与文件写入及 patch 模块使用的 `v1` 算法一致。

`maxLines` 仍作为模型主动选择读取范围的策略参数，但不再承担系统容量保护职责。真正的
模型投影上限只来自统一 10K budget，因此同样适用于 Python、Markdown、普通文本以及
未来接入的模型专用 tokenizer。Canonical Result、Event、Trace、Archive 和 Checkpoint
仍按各自消费者契约处理，不继承模型的 10K 限制。

## 容量保护与规划

```text
safety_margin = max(context_window * 5%, 1024)
available_input = context_window - reserved_output - safety_margin
```

每次 provider 请求前检查：

```text
fixed + durable + run_transient + request_only <= available_input
```

`ContextCompactionPlanner` 是无副作用纯函数，只读取同一报告和已经缓存 token 的 planning items。它不读取 SQLite、不调用模型、不修改 frame。

当前规则：

- 完整请求达到可用输入容量的 90% 时触发，运行时没有第二套 durable 压力；
- 触发后统一尝试把完整请求回落到可用输入容量的约 15%；
- 15% 是软目标；保护内容导致目标不可达时，仍压缩全部安全可压缩的连续前缀；
- 规划器的 replacement target 只用于预计回收量和提示模型，不参与 provider 输出截断；压缩没有专用固定上限，本次技术性 `max_tokens` 由统一容量报告、当前 run 输出配置和实际可缩小空间动态确定；
- fixed、request-only、附件、图片、runtime guard、最新用户指令和不可摘要运行状态绝对保护；
- tool call/result 闭环不可拆；
- 规划结果只有一个面向统一目标的最旧闭合日志前缀，不区分历史 turn 和当前 Agent Loop。

单个工具结果固定受 10K Model Result Gate 限制，并在可行动时明确返回截断状态、源长度
和可用 cursor。模型 observation 不携带后端 archive ID/hash；无法一次放入窗口的结果
通过工具分页或 `conversation_history` 的后端路由读取 Exact Archive，不会静默丢失。

## 会话状态、缓存和圆环

`AgentConversationContextState` 是后端唯一的会话级派生状态。它保存分类后的 `ContextFrame`、增量计量缓存、模型配置 revision 和当前 trace 追加游标。

状态把已测量长期内容冻结成共享不可变 `AgentContextBaseline`。新增消息、narration 或闭合工具结果只测量新增块，不复制或重测旧块。core-server 使用最多 32 个会话的 LRU 缓存。

以下情况会使缓存失效并从 SQLite 整体重建：

- 显式上下文压缩或摘要回退；
- 消息删除、编辑或回退；
- 会话或项目删除；
- 模型、窗口、工具定义或系统提示词配置变化；
- 无法安全应用增量 trace。

圆环不是独立功能实例。它调用同一个会话状态的 `snapshot()`，显示：

```text
complete_input / available_input
```

尚未发送第一条用户消息时，对话并未建立实际模型上下文，Core Server 明确发布 0 用量。第一条消息发送后，`complete_input` 立即包含系统契约、工具 Schema、初始 Run World State、Skill 目录、用户消息和当前 Agent Loop；固定内容不再扣除。

圆环与 Compaction 使用同一个 `ContextBudgetReport` 和相同的完整请求比例：低于 90% 不触发，达到 90% 时触发。前端百分比向下取整，避免 89.x% 提前显示为 90%。运行中 narration 或工具闭环成功落库后，状态增量计量并立即发送 `context_window_updated`。只有显式压缩、删除、回退或配置变化可以让它下降或重算。输入框尚未发送的草稿不参与计算。

关闭前端圆环只停止展示和事件，不改变后端状态、容量保护或压缩能力。

## Runtime Extension 的边界

Runtime Extension 可以注册工具、贡献 request-only 上下文、处理 runtime event，并在审批检查点保存状态。它不拥有 Agent loop，也不能直接替换上下文。

自动压缩位于“容量检测与主模型请求之间”，会阻塞当前 loop，因此由 runtime 的 `ContextCompactionExecutor` 直接编排 prepare、generate、commit，而不是伪装成普通工具或 extension。

## 当前限制

- 原始消息和安全清洗后的工具文本 Archive 不会因上下文压缩删除，长会话会增加 SQLite 占用；Archive 使用 zstd 分块降低该成本。
- Continuity V2 是固定配额的后端索引，不进入默认模型上下文；长会话的模型成本不会因
  Continuity ref 数量线性增长。
- 尚无项目级记忆和语义/向量检索。
- 尚未接入 provider 精确 tokenizer。
- 单个模型可见工具结果固定受 10K Gate 保护；只有超过 64 MiB 共享安全捕获硬限的来源
  后缀无法通过 Archive 恢复。
- 会话缓存按数量限制，尚未按内存或 token 总量限制。

## 主要代码位置

| 模块                | 路径                                                                   |
| ------------------- | ---------------------------------------------------------------------- |
| 上下文组装          | `crates/core/src/context/assembler.rs`                                 |
| ContextFrame 与分类 | `crates/core/src/context/frame.rs`                                     |
| 会话状态与共享基线  | `crates/core/src/context/state.rs`                                     |
| Token estimator     | `crates/core/src/context/measurement.rs`                               |
| 预算与容量检测      | `crates/core/src/context/budget.rs`                                    |
| 压缩规划器          | `crates/core/src/context/compaction.rs`                                |
| 摘要与稳定游标契约  | `crates/core/src/context/compaction_summary.rs`                        |
| 确定性连续性骨架    | `crates/core/src/context/continuity.rs`                                |
| Agent 原始 trace    | `crates/core/src/conversation_trace.rs`                                |
| 历史 trace 渲染     | `crates/core/src/context/trace_renderer.rs`                            |
| Agent tool loop     | `crates/core/src/runtime.rs`                                           |
| 压缩执行器          | `crates/core/src/runtime/context_compaction.rs`                        |
| 真实摘要生成器      | `crates/core/src/runtime/context_compaction_model.rs`                  |
| 审批检查点          | `crates/core/src/runtime/checkpoint.rs`                                |
| Trace 持久化        | `crates/core/src/storage/conversation_trace_repository.rs`             |
| 精确历史 Archive    | `crates/core/src/storage/conversation_history_archive_repository.rs`   |
| 历史检索工具        | `crates/core/src/tools/conversation_history.rs`                        |
| 原始历史查询        | `crates/core/src/storage/conversation_history_repository.rs`           |
| 历史查询工具        | `crates/core/src/tools/conversation_history.rs`                        |
| 文本文件分页读取    | `crates/core/src/tools/read_file.rs`                                   |
| 摘要原子提交        | `crates/core/src/storage/context_compaction_repository.rs`             |
| 任务分叉快照        | `crates/core/src/storage/conversation_fork_repository.rs`              |
| 请求计量观测        | `crates/core/src/model_request_observation.rs`                         |
| 压缩 receipt        | `crates/core/src/context_compaction_receipt.rs`                        |
| Observation 持久化  | `crates/core/src/storage/model_request_observation_repository.rs`      |
| Receipt 持久化      | `crates/core/src/storage/context_compaction_receipt_repository.rs`     |
| SQLite schema       | `crates/core/src/storage/migrations.rs`                                |
| 会话状态 LRU 与接线 | `crates/core-server/src/agent.rs`                                      |
| 前端圆环            | `src/renderer/src/features/chat/components/ContextWindowIndicator.tsx` |
