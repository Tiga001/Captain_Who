# 上下文管理架构

本文说明 MyCopilot 如何组装模型请求、保存跨轮 Agent 轨迹、维护会话级上下文状态、估算请求容量，以及前端上下文圆环如何自然地成为后端长期状态的一个只读投影。

## 设计目标

- 所有模型请求都从同一套 provider-neutral 上下文结构生成。
- 当前 tool loop 能看到完整、必要的工具结果，同时避免把大块临时数据永久写入历史。
- 下一轮模型能看到上一轮公开过程、工具调用和有限结果，不依赖前端 timeline JSON。
- 容量保护、未来压缩判断和前端圆环共用同一套分类与计量规则。
- 长期消息只测量一次；当前 run 在共享不可变基线上追加 overlay。
- 前端展示开关只能控制事件和 UI，不能控制后端上下文状态的生命周期。
- 审批暂停不会丢失当前运行上下文。
- 计量可以增量更新，并能在未来替换为模型专用 tokenizer。

当前已经实现无副作用的压缩规划、版本化摘要实体、durable 连续前缀快照、SQLite 原子替换层、阻塞式 `ContextCompactionExecutor`，以及使用当前 run 固定模型的真实摘要生成器。配置了模型上下文窗口后，系统可以完整走通“准备快照 -> 模型生成 -> 原子提交 -> 权威重建 -> 重新计量 -> 重新规划”。项目级记忆、语义检索和 run-overlay 压缩尚未实现。

## 总体数据流

```text
SQLite messages + ConversationTurnTrace + active ContextCompactionSummary
                    |
                    v
       durable context projection
                    |
                    v
             ContextAssembler
                    |
                    v
      AgentConversationContextState
       (durable items + measured cache)
             /                 \
            v                   v
 shared immutable baseline   persistent snapshot
            |                   |
            v                   v
 ContextFrame + run overlay  frontend indicator
            |
            v
 capacity report + compaction plan
            |
            v
 ContextCompactionExecutor (when required)
            |
            v
 authoritative rebase + remeasure + capacity gate + provider payload
```

`ContextAssembler` 是从持久化投影建立上下文的唯一结构化入口。它负责验证摘要和消息、恢复历史工具协议、添加系统提示词并生成 `ContextFrame`。存在 active summary 时，core-server 保留 SQLite 原始消息用于 UI 和审计，但只向 assembler 交付“摘要 + 未覆盖消息尾部”。core-server 随后把该 frame 提升为 `AgentConversationContextState`。OpenAI 与 Anthropic 的 wire payload 都从共享基线和当前 run overlay 生成，不从前端 timeline 反推。

SQLite `messages.created_at` 保存 Unix 毫秒时间戳。持久化消息进入 `AgentChatMessage` 后仍保留该字段；assembler 在请求边界按操作系统本地时区把它渲染成带 UTC 偏移的 RFC 3339 前缀，例如 `[Message created at: 2026-07-13T15:32:18.123+08:00]`。这与前端对同一时间戳的本地显示语义一致，同时保留完整的绝对时间信息；系统无法确定本地偏移时安全回退到 UTC。数据库原始正文不会被改写；时间前缀作为真实模型输入参与统一 token 计量和基线缓存。正文为空但带历史 trace 的 assistant 消息不会因时间前缀变成伪文本消息。压缩源同样携带这一时间，并以相同格式交给摘要模型。

## 上下文来源与顺序

一个历史 assistant turn 按以下顺序进入下一轮：

```text
assistant narration
tool call
tool result
assistant final reply
terminal record
```

新一轮 user 消息位于这些历史活动之后。

主要来源包括：

| 来源                         | 内容                                            | 是否跨轮           |
| ---------------------------- | ----------------------------------------------- | ------------------ |
| BackendSystemPrompt          | 后端系统提示词和工具行为约束                    | 每次重新生成       |
| ConversationSummary          | 已覆盖历史前缀的版本化压缩摘要                  | 是                 |
| ConversationHistory          | user/assistant 最终文本                         | 是                 |
| ConversationTrace            | 公开 narration、工具调用、有限工具结果、终态    | 是                 |
| CurrentTurn                  | 当前 user 输入                                  | 完成本轮后转为历史 |
| ModelResponse                | 当前 tool loop 中的模型输出与工具调用           | 仅当前运行         |
| ToolResult                   | 当前 tool loop 使用的完整或脱敏工具 observation | 仅当前运行         |
| InputAttachment              | 当前请求的附件正文或图片                        | 仅当前运行         |
| RuntimeExtension             | Todo 等扩展临时注入                             | 单次请求           |
| RuntimeGuard/FileTransaction | 协议纠正和文件事务约束                          | 单次请求或当前运行 |

## 分类模型

每个 `ContextItem` 都携带 `source`、`scope`、`retention` 和可选的原子工具交换分组。计量时映射为四类：

| 分类          | 含义                                | 主要消费者                         |
| ------------- | ----------------------------------- | ---------------------------------- |
| fixed         | 系统提示词、工具定义和请求协议开销  | 容量保护、压缩查询、圆环净容量预留 |
| durable       | 会话历史和持久化 trace              | 容量保护、压缩查询、圆环           |
| run_transient | 当前 tool loop 的模型输出和工具正文 | 容量保护、压缩查询                 |
| request_only  | Todo、文件事务提示等本次请求注入    | 容量保护、压缩查询                 |

圆环显示的是 `durable / durable_capacity`，其中：

```text
durable_capacity = available_input - fixed
```

系统提示词、工具定义和协议固定开销不计入“已用”分子，但会先从可供长期历史使用的分母中扣除。这样圆环表达的是对话历史与持久化 trace 实际占用了多少净长期容量。网页正文等临时 observation 不会让圆环先增加、运行结束后又减少；只有压缩、删除或回退等真正改写长期历史的操作允许圆环下降。

## ConversationTurnTrace

`ConversationTurnTrace` 是后端生成、provider-neutral、版本化的长期 Agent 活动记录。它不读取 `agent_run_json`，也不保存隐藏 reasoning。

一次工具执行只发生一次，但结果形成三种视图：

1. runtime observation：供当前 tool loop 使用，可包含较完整正文。
2. durable projection：限量、脱敏后写入 trace，供未来轮次使用。
3. presentation event：供前端 timeline 展示。

运行中的 trace 使用 append-only SQLite 记录：

- 稳定 narration 可以立即提交。
- tool call 必须等 tool result 闭合后一起提交。
- 已提交前缀不能重写、缩短或重新打开。
- assistant 最终消息、运行终态和 terminal trace 在同一事务中提交。

SQLite 表：

- `conversation_turn_traces`
- `conversation_turn_trace_items`

删除 assistant 消息或会话时由外键级联删除对应 trace。

## ContextCompactionSummary 与 durable 前缀替换

`ContextCompactionSummary` 是 provider-neutral、版本化、不可变的长期摘要实体。它记录：

- schema version、摘要 ID 和会话 ID；
- 生成摘要时绑定的 `sourceRevision`；
- 前一摘要版本 ID，用于递归压缩和回滚；
- 被覆盖的连续消息 ID 前缀及最后一个 assistant 边界；
- 摘要正文、生成方式、模型标识；
- 压缩前和摘要后的 token 估算；
- 创建时间。

摘要生成输出先以 `ContextCompactionSummaryDraft` 存在。草稿必须携带生成时的
`sourceRevision`；原子提交会拒绝把基于其他前缀生成的草稿写入当前会话。这样即使未来
真实模型摘要在事务外耗时生成，也不会把并发请求或旧历史的摘要错配到新前缀。

压缩不会删除或改写 `messages` 与 `ConversationTurnTrace`。SQLite 使用三张表维护上下文投影：

- `context_compaction_summaries`：不可变摘要版本；
- `context_compaction_summary_sources`：每个版本覆盖的有序消息前缀；
- `conversation_context_compaction_heads`：每个会话当前生效的摘要版本。

durable 前缀必须从 user 消息开始，并在一个已完成的 assistant turn 后结束；不能跨过 pending 消息或运行中的 trace。已有摘要时，新前缀只能继续向后扩展，不能缩短或改写旧覆盖范围。递归摘要生成只读取“当前摘要 + 新覆盖的尾部消息”，不会重新把已压缩的全部原文塞回模型。

每个 durable `ContextItem` 还携带稳定来源身份：原始消息使用 SQLite message ID，摘要使用
summary ID。历史 assistant 的 narration、tool call/result、最终回复和 terminal record 共用同一个
message ID，因此规划器会把整个 turn 视为不可拆分单元。计划中的 durable 步骤同时给出 active
summary ID、新覆盖消息 ID 和最后一个 assistant message ID；执行器不需要把易变的 frame 索引
反推成数据库边界。如果 token 目标恰好落在 user 消息后，规划器会继续延伸到对应 assistant
完成点。

原子替换流程为：

```text
短事务读取连续前缀并生成 sourceRevision
                    |
                    v
事务外使用当前模型生成摘要
                    |
                    v
写事务重新读取同一边界
  -> 校验 active head、消息顺序、trace 和 sourceRevision
  -> 插入不可变摘要版本及覆盖元数据
  -> 原子切换 active head
  -> commit
```

摘要生成期间历史发生变化时返回 `stale_context_compaction_prefix`。摘要校验、SQLite 插入或 head 切换任一步失败，事务都会回滚，旧 active head 保持不变。回滚操作也要求 expected summary ID，并原子恢复前一版本或原始历史。

被摘要覆盖的消息正文、角色、状态、创建时间、位置或 trace 后续发生变化时，SQLite trigger 会清除该会话的摘要版本，防止过期摘要继续携带已删除或已修改的信息。成功替换或回滚后，编排层必须使 `AgentConversationContextState` 失效；下一次读取从 SQLite 权威投影重建、重新计量并刷新圆环。

## 审批与完整运行检查点

审批会暂停当前 Rust 运行任务。暂停前保存：

- 完整 `ContextFrame`
- 下一次模型请求序号
- 待审批调用和后续工具队列
- 扩展快照，例如 Todo
- ConversationTrace recorder 状态

审批完成后恢复同一个 run 的检查点，先把审批结果作为工具结果补回上下文，再继续请求模型。计量缓存属于派生数据，不写入检查点；恢复后由 `ContextFrame` 根据 estimator identity 重建一次。

## 统一计量源

计量链分为三层：

1. `ContextTokenEstimator`：估算单条消息、工具定义、图片和协议开销。
2. `ContextFrame`：缓存每个 item 的估算并维护四类增量汇总。
3. `ContextCapacityDetector`：结合模型窗口、输出预留和安全余量生成 `ContextBudgetReport`。

一个 `ContextBudgetReport` 可以派生：

- 完整请求容量判断
- `ContextCompactionQuery`
- 长期 `AgentContextWindowSnapshot`

这些消费者不维护自己的 token 公式。

当前 estimator 是启发式实现：ASCII 约 3 字符/token，非 ASCII 约 2 token/字符，图片使用固定预留。接口已经支持 estimator identity、版本和整帧复核，但尚未接入模型专用 tokenizer。

## 容量公式

```text
safety_margin = max(context_window * 5%, 1024)
available_input = context_window - reserved_output - safety_margin
```

tool loop 在每次真正发送 provider 请求前检查完整请求：

```text
fixed + durable + run_transient + request_only <= available_input
```

超限时请求不会发送。`ContextCompactionQuery` 和 `ContextCompactionPlan` 都从同一个报告及其已缓存逐项计量生成。计划为 `required` 且宿主提供摘要生成服务时，runtime 会先阻塞执行 durable 压缩，然后基于新权威基线重新计量和规划；没有生成服务、没有可执行 durable 步骤，或压缩后仍超过硬上限时，才返回结构化容量错误。

## 压缩规划器

`ContextCompactionPlanner` 在每次 provider 请求完成上下文组装和计量之后、容量 gate 之前运行。它是纯决策模块：不读取 SQLite、不调用模型、不修改 `ContextFrame`，也不改变当前请求是否发送。

输入只有两部分：

1. `ContextCompactionQuery`：来自本次请求唯一的 `ContextBudgetReport`。
2. `ContextFramePlanningItem`：只包含分类、已缓存 token、角色、来源、原子分组和工具安全属性，不包含消息正文。

因此规划不会触发第二次 tokenizer 计量。输出 `ContextCompactionPlan` 包含：

- `not_required`、`required`、`insufficient_compactable_context` 等明确状态；
- 触发线、动态目标、必须回收量和预计回收量；
- request/durable 目标是否达到，以及是否为 `best_effort`；
- 受保护 token 及原因；
- 按 `run_overlay`、`durable_history` 划分的候选范围；
- 每一步允许的最大摘要输出 token；
- 是否包含副作用工具或错误记录。

规划器有两条相互独立但使用同一计量报告的压力通道：

1. 完整请求压力：`fixed + durable + run_transient + request_only` 达到可用输入的 75% 时触发，目标由当前 run 增长动态决定，负责避免 tool loop 被临时大结果撑爆。
2. 长期上下文压力：圆环同口径的 `durable / (available_input - fixed)` 达到 75% 时触发，目标直接设为净长期容量的 15%。

规划器会分别计算完整请求和 durable 的理想回收量，候选选择不能只压缩 run overlay 来假装长期目标已经完成。15% 是软目标：受保护内容过多时，规划器仍对所有能安全产生收益的候选生成 `required + best_effort` 计划，并明确报告预计压缩后仍高于目标。只有完全没有有效压缩步骤时才返回 `insufficient_compactable_context`。

摘要预算也不是“原文固定乘一个百分比”。规划器先计算本次必须回收的 token，再给摘要保留仍能达到目标的最大输出预算，并使用绝对输出上限防止摘要本身过大。后续执行器可以生成更短摘要，但不能超过计划预算。

计划里的所有 item range 都绑定同一个 `contextRevision`。执行器先根据稳定 message ID 向宿主取得只读 durable 前缀，再原子提交替换；不能先修改一段上下文，再拿已经偏移的旧索引继续修改。

候选选择遵守以下规则：

- fixed、request-only、当前 user 消息、附件、图片和 runtime guard 绝对保护；
- 同一个工具 call/result 分组不可拆开，混合分类的原子分组整体保护；
- 优先处理较旧的 run overlay，其次是较旧的 durable history；
- 最近内容、失败记录和有副作用的工具活动降低选择优先级，但不是永远不可压缩；
- 未知工具按有副作用处理；
- 安全候选无法达到理想目标时仍生成 best-effort 计划；没有任何候选能产生实际收益时才返回 `insufficient_compactable_context`。

设置 `MYCOPILOT_CONTEXT_MANIFEST=1` 时，现有上下文诊断会同时打印 capacity report、compaction query 和 compaction plan，供开发期核验。正常运行不会向前端新增事件。

## 会话状态与运行视图

系统不再为前端圆环维护独立计量器。后端只有一个通用的会话级派生状态：`AgentConversationContextState`。

它由 core-server 所有，事实来源是 SQLite，包含：

- 已确定的 fixed 与 durable `ContextItem`；
- 每个 item 的 estimator 结果和分类汇总；
- 模型、系统提示词、工具定义和预算配置的 revision；
- 活动 run/message 身份与已提交 trace 游标。

状态把已测量的长期内容冻结成按块共享的 `AgentContextBaseline`。块是不可变的，新增 user、narration 或闭合的 tool call/result 只形成新块，不复制或重测旧块。

`AgentRuntime` 启动时从 trace observer 获得同一份 baseline，并建立运行视图：

```text
运行视图 = shared durable baseline + current run overlay + request-only overlay
```

因此会话状态与运行时仍有不同生命周期，但不再是两套独立计量：长期内容和长期分类汇总由 Arc 共享。运行时只测量当前附件、模型输出、工具结果、Todo 和文件事务提示等增量。

审批恢复时，检查点会与当前 baseline 按不可变块匹配。匹配的长期前缀直接复用，只有检查点中的 run overlay 重新测量；配置或内容不一致时安全回退为完整重建。

## 会话级状态缓存

core-server 使用最多 32 个会话的 LRU 缓存。缓存项包含 `AgentConversationContextState`、配置 revision、活动 run/message 身份和已提交 trace 游标。

缓存命中时：

- 新 user 消息测量一次。
- 新 narration 只测量新增项。
- 新工具调用只在 call/result 闭合提交后测量。
- 最终 assistant 回复和 terminal record 各测量一次。
- 新 run 直接共享已经测量的长期 baseline，不再重新扫描历史。

以下情况使状态失效并在下次运行或读取时从 SQLite 整体重建：

- 消息删除或回退
- 会话或项目删除
- 模型配置变化
- Agent 提示词配置变化
- 强制取消导致长期终态改变
- 未来的上下文压缩或其他历史重写

配置 revision 覆盖模型、API 风格、窗口、输出预留、系统提示词和工具 definitions。前端圆环开关不属于 revision，也不会删除状态。

## 前端同步

前端在切换会话、模型、权限或上下文展示设置时主动请求一次快照。运行中，core-server 在 durable trace 成功提交后可发送 `context_window_updated`。

该事件来自 `AgentConversationContextState.snapshot()`，只是 durable 使用量和净长期容量的展示投影。关闭圆环只会停止查询和事件发送；会话状态仍为 runtime 容量保护和未来压缩服务。

前端同时维护请求序号和事件序号，防止较慢的主动查询覆盖更新的运行事件。输入框尚未发送的草稿不参与圆环计算。

## Runtime Extension 与压缩执行器

Runtime extension 当前支持：

- 注册工具
- 为单次模型请求贡献上下文
- 处理已完成的 runtime event
- 在审批检查点中保存和恢复扩展状态

Runtime extension 不拥有 Agent loop，也不能直接替换上下文。这个约束避免 Todo、监控或未来记忆扩展与 runtime 同时改变控制流。压缩会阻塞一次 provider 请求并要求从请求准备阶段重新开始，因此由 runtime 直接持有 `ContextCompactionExecutor`，而不是把它实现成普通 extension。

执行位置固定为：

```text
克隆 active ContextFrame
  -> 注入 RuntimeExtension request-only context 和 runtime guard
  -> 统一容量检测
  -> 纯函数压缩规划
  -> ContextCompactionExecutor
  -> 权威基线替换并回到“克隆 active ContextFrame”
  -> 容量 gate
  -> provider 请求
```

执行器使用三个可独立注入、可测试的阶段：

1. `prepare`：宿主依据 message ID 和 expected active summary 从 SQLite 取得不可变 durable 前缀；若计划已过期，返回新的权威 baseline，不生成摘要。
2. `generate`：在事务外使用当前 run 固定的模型生成绑定 `sourceRevision` 的摘要草稿，并返回该次模型用量；测试可以注入确定性替身。
3. `commit`：SQLite 写事务再次校验 active head、消息顺序、trace 和 source revision，原子切换摘要 head，然后从 SQLite 重建共享 baseline 与圆环快照。

每个阶段都接收同一个 cancellation token，runtime 还会用异步选择立即停止等待。生成期间取消不会写 durable 状态；提交成功后即使运行随后取消，摘要仍是合法的长期状态。prepare 或 commit 发现 stale 时不算运行失败，而是返回权威 baseline，重新计量并规划。单次 provider 请求最多重基三次，防止外部状态持续变化造成内部循环。

真实摘要生成器只保存本次 run 的 API 地址、认证、模型、API 风格和窗口配置。它直接复用普通模型请求层的网络重试、超时、取消和 usage 提取，但使用独立系统提示词、空 tools，并遵循当前 run 的流式配置，不会启动子 Agent，也不会递归触发压缩。输入是“上一版摘要 + 新覆盖消息”的结构化 JSON；历史内容被明确标记为不可信数据，不能反向覆盖压缩指令。摘要未来进入主上下文的 token 占用由统一计量器计算，不能用 provider 输出 token 替代。模型因 `length` 或 `max_tokens` 截断、返回工具调用、返回空正文或超过规划预算时，草稿不会提交。

runtime 在真实执行器前后发送瞬时 `context_compaction_started` / `context_compaction_finished` 事件。前端只在执行期间显示“正在自动压缩上下文”，事件不进入永久 timeline。

压缩后的 `ContextFrame` 只替换 fixed/durable 基线，保留当前 run 的 narration、工具 call/result 等 overlay。若随后遇到审批，完整 frame 会进入现有运行检查点；恢复时继续使用摘要和 run overlay，不会重新装回已被摘要覆盖的原始历史。

当前执行器只消费规划中的 `durable_history` 步骤。规划器已经能识别 run-overlay 压力，但运行内摘要与替换尚未实现。真实摘要生成器直接构造受控的内部请求上下文，不接收 todo 等 runtime extension 的请求注入，也不会把 Agent loop 控制权交给 extension。

## 当前限制与后续方向

- 尚未实现 run-overlay 摘要与替换；纯当前运行的巨大工具结果仍可能触发结构化容量错误。
- 尚无项目级记忆；`ContextScope::Project` 仅保留类型位置。
- 尚无语义相关片段检索。
- 启发式 token 计量不是 provider 精确 tokenizer。
- 会话缓存按数量限制，尚未按估算内存或 token 总量限制。
- 历史改写后的缓存失效目前由 core-server 调用方显式触发，未来应收敛为统一的 durable-history mutation 入口。

## 主要代码位置

| 模块                 | 路径                                                                   |
| -------------------- | ---------------------------------------------------------------------- |
| 上下文组装           | `crates/core/src/context/assembler.rs`                                 |
| ContextFrame 与分类  | `crates/core/src/context/frame.rs`                                     |
| 消息时间格式化       | `crates/core/src/context/message_time.rs`                              |
| 会话状态与共享基线   | `crates/core/src/context/state.rs`                                     |
| Token estimator      | `crates/core/src/context/measurement.rs`                               |
| 预算、容量与压缩查询 | `crates/core/src/context/budget.rs`                                    |
| 压缩规划器           | `crates/core/src/context/compaction.rs`                                |
| 压缩摘要契约         | `crates/core/src/context/compaction_summary.rs`                        |
| 历史 trace 渲染      | `crates/core/src/context/trace_renderer.rs`                            |
| Agent tool loop      | `crates/core/src/runtime.rs`                                           |
| 压缩执行器           | `crates/core/src/runtime/context_compaction.rs`                        |
| 真实摘要生成器       | `crates/core/src/runtime/context_compaction_model.rs`                  |
| 审批检查点           | `crates/core/src/runtime/checkpoint.rs`                                |
| Runtime extensions   | `crates/core/src/runtime/extensions/`                                  |
| Trace 持久化         | `crates/core/src/storage/conversation_trace_repository.rs`             |
| 摘要原子替换         | `crates/core/src/storage/context_compaction_repository.rs`             |
| 会话状态 LRU 与接线  | `crates/core-server/src/agent.rs`                                      |
| 前端圆环             | `src/renderer/src/features/chat/components/ContextWindowIndicator.tsx` |
