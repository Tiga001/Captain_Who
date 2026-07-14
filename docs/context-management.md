# 上下文管理架构

本文说明 MyCopilot 当前的上下文事实来源、模型请求组装、运行中 Agent 轨迹、统一计量、自动压缩和前端上下文圆环。

## 核心原则

系统只维护一份逻辑上的原始上下文日志：

```text
用户/助手消息 + 助手公开过程 + 工具调用 + 工具结果 + 运行终态
```

所有消费者都从这份日志派生自己的视图：

- 主模型看到“系统提示词 + 当前摘要 + 摘要游标后的原始日志 + 本次请求临时内容”。
- 压缩器读取“上一版摘要 + 游标之后待压缩的原始日志前缀”。
- 容量保护和上下文圆环读取同一个 `ContextFrame` 分类计量结果。
- 前端 timeline 使用展示事件，但展示事件不是模型上下文的事实来源。

压缩不会删除原始消息或 Agent 轨迹，只会生成一版摘要并向后移动一个稳定游标。删除消息、回退或删除会话才会改变原始日志。

系统明确不实现第二套 observation 仓库、工具结果索引或 run-overlay 专用压缩路径。

## 总体数据流

```text
SQLite messages + ConversationTurnTrace
                    |
                    v
         logical context journal
                    |
          +---------+---------+
          |                   |
          v                   v
active summary cursor     raw suffix after cursor
          |                   |
          +---------+---------+
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

消息时间使用 SQLite `messages.created_at` 的 Unix 毫秒值。组装请求时按本机时区渲染为带 UTC 偏移的 RFC 3339 前缀；数据库正文不被改写，时间前缀参与统一 token 计量。

## ConversationTurnTrace

`ConversationTurnTrace` 是后端生成、provider-neutral、append-only 的 Agent 活动日志，不保存隐藏 reasoning。

Trace 保存主模型实际使用的文本上下文：

- narration 保存公开过程说明；
- tool call 保存模型生成的完整文本参数；
- tool result 保存主模型收到的完整文本结果或错误；
- Base64、data URL 等二进制内容在统一边界移除；
- 不按工具类型制作另一份长期摘要，不做语义去重，不因长度静默丢弃文本。

同一次工具执行只产生一份规范化的模型结果。当前 tool loop 和后续历史重建使用同一份结果；前端事件可以移除 `write_file.tail` 等纯展示不需要的字段，但不能反向成为模型上下文。

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
+ 当前 ContextCompactionSummary（如果存在）
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
- 摘要正文、生成模型、token 估算和创建时间。

SQLite 只需要两张摘要表：

- `context_compaction_summaries`：不可变摘要版本。
- `conversation_context_compaction_heads`：会话当前生效的摘要。

原始日志本身已经表达覆盖范围，因此不再维护 `context_compaction_summary_sources` 之类的消息 ID 镜像表。

多次压缩时，摘要模型读取：

```text
上一版摘要 + 上一游标之后、本次新覆盖的原始日志项
```

它不会重新读取已经被上一版摘要覆盖的全部原文。原始数据仍保留在 SQLite，供审计、回退和派生状态失效后校验。

## 原子压缩流程

```text
容量检测
  -> 纯规划器选择一个安全日志前缀
  -> prepare 从 SQLite 读取前缀并计算 sourceRevision
  -> generate 在事务外调用当前 run 固定模型
  -> commit 在写事务内重新读取同一前缀
  -> 校验 active head、游标和 sourceRevision
  -> 插入摘要并原子切换 head
  -> 从 SQLite 权威日志重建基线
  -> 重新计量、重新规划，再决定是否发送主请求
```

摘要生成期间原始前缀或 active head 变化时，本次草稿不会提交，而是刷新权威基线并重新规划。摘要提交失败不会改变旧 head。

原始日志在摘要游标之后继续追加，不会让摘要失效；游标覆盖范围内的消息被编辑、删除或回退时，`sourceRevision` 校验失败，派生摘要被丢弃并回到原始日志。

## 模型已见边界与审批恢复

工具结果必须至少进入一次成功的主模型请求，才能参与压缩。

Runtime 使用 `visible_trace_item_count` 记录当前 run 中主模型已经看到的 trace 前缀。SQLite 可以继续追加后续内容，圆环也可以立即增长，但规划器只接收已见前缀作为 durable 候选。

审批暂停前的 `AgentRunCheckpoint` 保存：

- 完整 `ContextFrame`；
- 下一次模型请求序号；
- 待审批调用和剩余工具队列；
- Runtime Extension 快照；
- ConversationTrace recorder；
- `model_visible_trace_item_count`。

最后一项不能根据“当前有多少闭合工具结果”猜测。模型一次发出多个工具调用时，前几个结果可能已经完成，但审批发生前尚未发回模型。恢复时必须使用暂停前的真实已见游标。

## 统一分类与计量

每个 `ContextItem` 都携带来源、scope、retention、稳定 origin 和可选工具原子分组。计量映射为四类：

| 分类          | 含义                                         | 压缩行为                 |
| ------------- | -------------------------------------------- | ------------------------ |
| fixed         | 系统提示词、工具定义、provider 协议开销      | 不压缩                   |
| durable       | 当前摘要、原始消息、模型已见的持久化 trace   | 可按稳定日志前缀压缩     |
| run_transient | 当前 run 中尚未被主模型成功接收的内容        | 保护，不走第二套压缩路径 |
| request_only  | Todo、文件事务提示等只服务本次请求的临时内容 | 不写入长期日志           |

计量链只有一套：

1. `ContextTokenEstimator` 估算消息、工具定义、图片和协议开销。
2. `ContextFrame` 缓存每个 item 的估算并增量维护分类汇总。
3. `ContextCapacityDetector` 生成一个 `ContextBudgetReport`。

同一个报告派生：

- provider 请求容量判断；
- `ContextCompactionQuery`；
- 前端 `AgentContextWindowSnapshot`。

消费者不维护自己的 token 公式。

当前 estimator 是启发式实现，并已预留 estimator identity 和整帧复核接口，未来可以替换为模型专用 tokenizer。

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

- 完整请求或 durable 使用量到达 75% 时可以触发；
- 期望把 durable 压到净长期容量的约 15%；
- 15% 是软目标，无法达到时仍执行有实际收益的 best-effort 压缩；
- fixed、request-only、附件、图片、runtime guard、当前请求首次发送前的用户消息绝对保护；
- run-transient 内容绝对保护；
- tool call/result 闭环不可拆；
- 规划结果只有一个连续 durable 日志前缀，不再区分 durable-history 和 run-overlay 两种执行步骤。

单个尚未被模型看到的工具结果如果自身超过窗口，系统不会把它偷偷摘要后再假装模型读过，而是返回结构化容量错误。此类问题应在工具契约层通过分页、分块或合理输出上限解决。

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
durable / (available_input - fixed)
```

运行中 narration 或工具闭环成功落库后，状态增量计量并立即发送 `context_window_updated`。因此圆环会随确定的长期日志单调增加；只有显式压缩、删除、回退或配置变化可以让它下降或重算。输入框尚未发送的草稿不参与计算。

关闭前端圆环只停止展示和事件，不改变后端状态、容量保护或压缩能力。

## Runtime Extension 的边界

Runtime Extension 可以注册工具、贡献 request-only 上下文、处理 runtime event，并在审批检查点保存状态。它不拥有 Agent loop，也不能直接替换上下文。

自动压缩位于“容量检测与主模型请求之间”，会阻塞当前 loop，因此由 runtime 的 `ContextCompactionExecutor` 直接编排 prepare、generate、commit，而不是伪装成普通工具或 extension。

## 当前限制

- 原始文本日志不会因压缩删除，长会话会增加 SQLite 占用；这是审计完整性与运行简单性的明确取舍。
- 尚无项目级记忆和语义检索。
- 尚未接入 provider 精确 tokenizer。
- 单个不可拆、尚未被主模型看到的超大工具结果不能由上下文压缩补救。
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
| Agent 原始 trace    | `crates/core/src/conversation_trace.rs`                                |
| 历史 trace 渲染     | `crates/core/src/context/trace_renderer.rs`                            |
| Agent tool loop     | `crates/core/src/runtime.rs`                                           |
| 压缩执行器          | `crates/core/src/runtime/context_compaction.rs`                        |
| 真实摘要生成器      | `crates/core/src/runtime/context_compaction_model.rs`                  |
| 审批检查点          | `crates/core/src/runtime/checkpoint.rs`                                |
| Trace 持久化        | `crates/core/src/storage/conversation_trace_repository.rs`             |
| 摘要原子提交        | `crates/core/src/storage/context_compaction_repository.rs`             |
| SQLite schema       | `crates/core/src/storage/migrations.rs`                                |
| 会话状态 LRU 与接线 | `crates/core-server/src/agent.rs`                                      |
| 前端圆环            | `src/renderer/src/features/chat/components/ContextWindowIndicator.tsx` |
