---
status: current
audience: developers
owner: engineering
last_verified: 2026-09-09
---

# Agent Runtime 与模型 Provider

本文描述 Rust Core 中一次 Agent Run 如何被创建、暂停、恢复和结算，以及模型 Provider 如何在不改变 Agent 语义的前提下接入。本文只描述后端权威状态；上下文压缩见[上下文管理](./context-management.md)，Tool 持久投影见[Conversation Trace 与 Exact Archive](./conversation-trace-and-archive.md)，文件写入见[FileChange 子系统](../subsystems/file-change.md)，数据库边界见[SQLite 存储与数据生命周期](./storage-and-data-lifecycle.md)。

## 职责边界

| 层                      | 负责                                                                                                          | 不负责                                                         |
| ----------------------- | ------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------- |
| Core Server application | 预留会话 Turn、解析请求、绑定权限和模型、持久化 pending action、执行 Core Server-owned 副作用、恢复与终态提交 | Provider wire payload、Tool 业务实现                           |
| `AgentRuntime`          | Provider-neutral Tool 循环、上下文容量判断、模型请求、Checkpoint、Tool 投影、取消与 Steering                  | UI 状态、数据库连接生命周期、直接执行需 Core Server 授权的动作 |
| LLM adapter             | 将 `LlmMessage`、Tool 定义和 continuation 映射为指定 Provider 的请求与响应                                    | 决定权限、压缩策略、重试后重复副作用                           |
| LLM transport           | HTTP、流式读取、错误归类、退避、超时和 Usage 合并                                                             | 修改 Provider 语义或补造 Tool Result                           |
| ToolRegistry            | Tool 身份、暴露、输入 schema、权限声明、执行和消费者投影                                                      | 持久 pending action 的生命周期                                 |

Scheduled Automation 不实现第二套 Runtime。AutomationScheduler 在持久 admission 后启动普通 HumanRoot Turn，
复用本表全部 Provider、Tool、Approval、Trace、Checkpoint、Usage 和取消语义；差异仅在 Host 冻结的执行上下文、
目标 Conversation 选择，以及该 Run 专用的 `automation_report` Tool。详见
[Scheduled Automation](../subsystems/scheduled-automations.md)。

Rust Core 与 Core Server 的边界是有意的：Runtime 可以提出动作并生成可恢复状态，但文件写入、命令、Office、Skill 安装、MCP Server 审批等权威副作用必须由 Core Server 编排。

## 关键对象

- `AgentRuntime`：一次逻辑 Run 的执行器。当前有 10,000 次 Tool 迭代保险上限，每个主请求最多尝试三次上下文压缩；这些是防失控边界，不是产品配额。
- `AgentRuntimeHostServices`：注入 Storage、Trace/Context observer、Compaction、Provider continuation vault、Skills、Office、图像生成、MCP Server、Command Session、Steering 与协作服务。
- `ProviderRegistration`：Provider 能力的唯一注册真源。UI 描述与运行时语义分开，禁止把 replay、checkpoint 或 usage 策略塞进 UI 配置。
- `ProviderProfile`：用户可选的模型端点和设置，带 schema/revision；它不是运行时能力判定的真源。协议语义使用 `provider_protocol_revision`，URL/credential 等连接身份使用独立的 `provider_connection_revision`，两者不能互相替代。
- `EffectiveToolSet`：在权限、运行时可用性、Skill/Capability 激活后冻结的本次请求 Tool 契约。
- `AgentRunCheckpoint`：审批或恢复边界保存的安全状态。Checkpoint 的 schema 是版本化协议，不能直接序列化任意运行内存。

## 当前 Provider

注册表当前包含以下 profile：

| Profile key                  | Adapter                    | 主要 wire 语义                                 |
| ---------------------------- | -------------------------- | ---------------------------------------------- |
| `generic_openai_chat`        | Generic OpenAI Chat        | OpenAI-compatible messages/tool calls          |
| `generic_anthropic_messages` | Generic Anthropic Messages | Anthropic-compatible content blocks/tool use   |
| `deepseek_v4_chat`           | DeepSeek V4 Chat           | DeepSeek reasoning continuation 与成组工具交换 |
| `moonshot_k3_chat`           | Moonshot K3 Chat           | Provider reasoning continuation 与成组工具交换 |
| `moonshot_k2_7_code_chat`    | Moonshot K2.7 Code Chat    | Provider reasoning continuation 与成组工具交换 |
| `moonshot_k2_6_chat`         | Moonshot K2.6 Chat         | 按 thinking 配置选择 reasoning 回放范围        |

此表用于读者定位，不应成为新的注册真源。新增或删除 Provider 必须以 `provider_registration.rs` 和 provider contract test 为准，并同步本文。

Provider 能力通过显式枚举描述，包括：Tool 交换方式、私有 replay、上下文投影、Usage 口径、partial Trace、Tool Call 来源、终态批次、Checkpoint 私有参数以及 continuation 要求。Runtime 应查询 `ProviderTurnRuntimePolicy`，不得按 profile 名称散落条件分支。

## 一次 Run 的主流程

```text
用户请求或 AutomationScheduler 预留 Conversation Turn
  -> 读取会话、模型、权限和可用服务
  -> 构造 Runtime input 与 Provider-neutral 历史
  -> 恢复可选 Checkpoint / Provider continuation
  -> 建立 World State epoch 与逻辑 ContextFrame
  -> 每次自然请求边界冻结能力策略快照、EffectiveToolSet 和 World State diff
  -> Host 原子写 Conversation 状态与请求回执（无变化不追加 diff）
  -> 投影请求布局并计量 ContextFrame
  -> 必要时压缩并从 SQLite 重建上下文
  -> 发送模型请求
      -> narration / final text
      -> 一批 Tool Calls
  -> Tool 调用分为
      -> Runtime 可直接执行
      -> 需要 Core Server 执行或用户审批
  -> 每个闭环生成 Model/Event/Trace/Archive/Checkpoint 投影
  -> 继续下一次模型请求，或暂停为 pending action
  -> 原子提交模型生成的 assistant message、Trace 终态、Usage/UI 终态
  -> 释放 Turn reservation
  -> 发布成功完成的终态事件
```

模型返回一批 Tool 调用时，Runtime 保留调用顺序与 Provider 要求的批次语义。Tool 调用不能在未形成权威结果时被当作已完成；需要审批的调用在 Checkpoint 中冻结，批准后由 Core Server 从持久状态恢复，而不是重新让模型生成一次。

普通回复与审批续跑的成功 `Done` 在终态提交、旧 Run 占用与并发许可释放、上下文清理之后发布。接收端可以据此请求下一轮，仍由 Turn admission 原子检查是否允许启动；等待审批或用户输入的 `Done` 不表示会话空闲。

Turn admission 创建的 pending assistant message 正文为空；系统不再把“正在思考”之类 UI placeholder 写进消息。流式 narration/final content 与终态正文只能来自模型输出或明确的错误/取消结算路径，展示占位符不得进入可压缩历史或被当作模型主张。

## 消息布局与动态能力

主请求分开提供 Provider-neutral 消息、`tools` 和模型参数。消息在发送前按下列顺序投影；完整边界见[上下文管理](./context-management.md#组装规则)：

```text
稳定 system
→ 初始 Skill 目录
→ 当前可用能力指南（RequestOnly，含启用时的协作规则与目录）
→ Conversation full World State
→ 摘要
→ 旧历史及 anchored diff
→ 本次连续用户输入及其 anchored diff / 附件
→ 初始预激活 Skill 完整说明
→ 初始 Run full World State
→ 因果 Run 时间线（含已发生的普通后端状态事件）
→ Todo / 单次修复提示 / 未完成文件事务的最小状态（若存在）
```

此次只把目录和当前指南移到 Conversation full 前，并把本次输入连同关联 diff、附件移到预激活 Skill 与初始 Run 状态前。作用域、权限判定和持久化规则不变；原消息正文、role、lifetime、retention、Tool 参数、调用与结果绑定、Provider continuation 也不因布局而改变。Run 中新激活的 Skill、审批或提问恢复后的 full World State、状态 diff、同步答案 ToolResult 与异步答案 User 消息保持因果位置；不能按类型提前到初始说明。同步答案虽可显示为用户气泡，模型仍只通过原工具调用的唯一结果接收。

Run 结束时，本次输入、预激活 Skill、初始 Run 状态及 live 时间线（完整布局第 8–11 项）仍不会整块历史化。后续 Run 只按原有日志规则重建用户消息、已结算输出、Tool 交换与状态记录；初始 Run 说明和状态块不会因发送位置改变而成为会话历史。

工具实现注册、模型工具暴露和使用指南是三层不同事实：实现可以保持已注册以支持同 Run 后续开启，`EffectiveToolSet` 决定本次 Schema，扩展贡献当前使用指南。`tools` 顺序仍为稳定工具按名称排序，再拼接动态工具按名称排序；稳定工具指导留在稳定 system，可选能力指南使用独立的 `CapabilityInstructions` 布局标记。`tools` 是消息之外的字段，其 JSON 属性位置不表示模型 token 顺序。

浏览器、联网搜索与人机交互在每次自然模型请求边界读取一致策略快照，同时投影 Schema、指南和 World State。关闭后下一次请求撤下对应工具与指南，World State diff 在发生位置记录不可用原因；保留的历史调用、结果和状态变化不重新授予工具权限或挂回说明。关闭无法撤回已发出的 Provider 请求，迟到调用在 Host 执行边界再次校验。开启浏览器也不替代当前任务审批；关闭人机交互不影响已接纳问题的提交、忽略与恢复。开关变化或忽略问题本身不产生额外模型请求。

多智能体复用相同的动态 Schema、RequestOnly 指南和 World State 投影机制，但策略在 Turn admission 冻结，并由本轮委派的 Wake 继承。关闭子 Agent 总开关不改变已经启动的任务树，新的根 Turn 才采用新设置；审批与进程恢复读取同一持久策略。协作六工具从稳定前缀移至动态后缀，预览和真实请求通过同一个扩展生成说明与目录，从而统一圆环及自动压缩的完整请求计量。

联网搜索、人机交互和浏览器用户开关属于跨 Run 的 Conversation 日志；Run full 只投影本任务浏览器激活、Skill 与附件状态，工具名称清单不再投影给模型。`AgentConversationWorldStateHost` 提供受信任的 prepare/observed 两个边界：请求发出前持久化并 CAS 校验，Provider 成功后确认观察；失败不重放模型。请求 anchor 定位到 assistant 内最后安全 Trace 前缀，压缩/分支不能只按 message 粗略归并。没有持久 Host 的嵌入式 Rust Core 使用相同内存日志，并随 checkpoint 携带 canonical 状态；预览只计算副本，不创建回执。

Adapter 继续执行各自 wire 规则：稳定 system 可投影为 OpenAI-compatible 的 system 消息或 Anthropic 顶层 system，普通后端状态按既有规则转为对应角色/内容块；Tool 交换仍依 Provider policy 投影为 split 或 grouped batch，continuation 绑定原助手回合。布局层不改这些适配规则。

目标是增加不变内容形成相同前缀的机会，不保证缓存命中或命中率提升。目录、能力或授权改变时，从变化位置开始的缓存可能失效；不能为了缓存继续发送过期能力。本轮没有新增 `cache_control`，也不保证厂商内部缓存组合或实际命中率达到某个最大值。

## Skill 专用说明的归属

Office 与 PDF 的专用读取、Builder/Editor 调用、运行环境选择、产物观察和私有工作目录规则，由对应内置 `SKILL.md` 正文持有。完整与极简模式的稳定 system、常驻工具描述及附件预处理状态仅保留通用约束，不按 Skill 开关或激活状态拼接、替换专用说明。关闭的 Skill 不进入当前可用目录；开启后仅公开目录元数据；激活后经现有 `backend_activated_skill` 上下文消息提供正文，稳定 system 与已提供的工具定义不变。

通用命令的参数结构和枚举（包括 `runtimeProfile`、`observe`）、执行器及权限审批契约保持原样。参数存在不代表对应 Skill 已开启；专用用法由已激活正文说明。已有历史内容不在这次迁移中删除，描述和 Skill 正文更新仍服从原有版本及检查点校验。

## 极简上下文模式

`AgentContextProfile` 的 `full` / `minimal` 独立于工作模式和扩展开关。完整模式保留原基础提示词与工具定义；极简模式使用独立短提示词，并仅压缩所选基础工具的描述，参数、必填项、枚举、校验约束、执行器及审批逻辑共用原契约。工具描述仍参与 ToolSet revision，必须在稳定工具集冻结前完成投影，不能在审批续跑或 Skill 激活后改写稳定 Schema。

极简文案按职责去重：系统提示词保留信任、权限、模式说明、Skill 作用域及跨工具工作流；参数、文件凭据与事务生命周期、命令会话状态等调用细节由对应原生工具说明提供。二者作为同一请求联合验证，规则移动不代表删去安全约束；完整模式文案、工具身份、返回格式、前端展示及存储协议不随极简文字优化改变。描述更新仍会改变极简 ToolSet revision，旧版暂停检查点的严格恢复校验不因此放宽。

极简模式保留 12 个基础入口：9 个核心工具 `read_file`、`read_image`、`apply_patch`、`run_command`、`command_session`、`workspace_map`、`search_files`、`search_code`、`conversation_history`，以及 `skills_activate`、`attachments_list`、`attachments_list_project`；不暴露 `todo_update`。这不是总工具数上限：扩展仍按原有设置、目录和授权路径提供自己的工具与指南，基础入口也继续受当前权限校验。

模式在根 Turn admission 时持久冻结，委派 Wake 继承来源 Run 的模式；已有 Run 和其任务树不随全局设置切换。实际生效模式通过 World State 的 `interaction.profile.contextProfile` 及简短说明投影，模型不能根据旧历史或工具数量猜测。模式选择、短提示词和工具投影同时用于真实请求及上下文预览；计量边界见[上下文管理](./context-management.md#容量判断)。

## Checkpoint 与恢复

Checkpoint 至少绑定以下事实：

- conversation、Run、assistant Turn 和模型身份；
- 已闭合的 Provider-neutral 历史以及尚待处理的调用批次；
- 冻结 Tool 定义、Tool 身份、能力与权限相关投影；
- World State epoch、扩展快照及可恢复的 Provider continuation 引用；
- canonical Conversation World State 日志及已观察标记（checkpoint v15、私有恢复信封 v12），恢复时校验与冻结模型投影一致；
- 上下文布局标签与可选 `request_order`，用于恢复压缩后 journal 项和 Run overlay 的相对请求次序；
- 审批动作所需的安全投影，而不是任意原始 secret 或不可信参数。

FileChange 的恢复状态横跨两类私有存储：checkpoint 保存 pending/queued Observation、模型已观察的 successor 和可选 Run grant ref；Host pending action/audit/Staged store 保存 exact proposal binding 与待处理 transaction。Observation/run-grant ref 都不是独立 authority：恢复与 effect boundary 必须从 Host 持久 action/audit/grant 重新验证 owner、revision、scope 和 receipt；Event/Trace/Archive 不得提供这些字段。

恢复时必须重新验证版本、归属、调用 ID、Tool 身份、权限和 Provider 能力。外部 MCP Server 调用与未知 Tool 不能仅凭普通 Checkpoint 保存其原始参数；它们需要各自的授权信封或明确失败。Tool 定义在暂停后变化时，应遵循冻结契约或返回结构化恢复错误，不能静默按新定义执行旧调用。

`request_order` 只是私有检查点中的布局元数据，不是 journal sequence、权限身份或可执行授权。压缩重建从权威日志恢复已经闭合的消息，通过此元数据维持它们与尚存 Run overlay 的因果相对顺序；恢复时不重新排列已发生的工具队列、回答与状态变化。当前能力指南仍在下一次自然采样时重新生成，不从 checkpoint 恢复旧的开启状态或 RequestOnly 文本。

模型请求只解析所选 connection 所需的 secret，并在 SQLite transaction/mutex 之外访问 Credential Store。Run、子 Agent Wake、Automation snapshot 和恢复输入必须冻结并复核 `provider_connection_revision`；凭据替换或清除会改变连接 revision，旧 Run 不得静默使用新密钥。catalog/usage/模板等只需元数据的路径不得批量解密凭据，也不能因一个不相关 reference 不可用而阻断全部模型。

审批 ticket 与 sealed/process execution material 的生命周期分开：MCP Server、Browser risk、内置敏感 Tool 和 Skill 安装即使临时 payload 已过期，ticket 也不会因此自动结算；它仍等待用户决定或所属 Run 的取消/终态流程收口。晚批准时若 Host 已无法取得精确材料，continuation 必须接收 definitely-not-dispatched 的 failed Tool Result 并继续模型闭环，不能重新生成调用、自动 retry 或声称用户决定已过期。

## Provider continuation

DeepSeek 等 Provider 可能要求在后续工具请求中回放 Provider-native 私有片段。系统将这类状态与公开消息、Trace 和模型上下文分开：

1. Adapter 从已验证响应生成 `ProviderContinuation`，并绑定 Turn digest、位置和 Tool Call identity。
2. Vault 使用受保护凭据加密，压缩后存入 SQLite；日志、Renderer 事件和公开 archive 不得包含明文。
3. continuation 先处于不可 replay 的准备状态；只有关联 Turn 成功持久化后才提升为可 replay。
4. 恢复、分叉和 Provider 切换都校验 conversation/Run/message/profile/revision 归属。
5. 删除、重写或不再需要时释放引用。缺失、损坏或凭据不可用时 fail closed，不降级为猜测 replay。

Provider 切换由 Core Server 的 transition 流程记录。若目标 Provider 无法消费现有私有状态，应先执行明确的上下文适配/重写，或阻止继续；不得把 DeepSeek 私有 reasoning 当成普通 assistant 文本发送给其他 Provider。

## 重试、超时与取消

- LLM transport 当前每个逻辑请求最多三次尝试，并按 rate limit、overload、网络/流式错误使用有上限的退避和总睡眠预算。
- 流式请求同时受请求超时和 inactivity timeout 约束。已收到不完整 Tool arguments 时，只有明确判定为安全、可修复的 Provider 错误才可重试。
- 多次网络尝试的 Usage 会合并为同一逻辑请求观测，`billableRequestCount` 保留真实计费尝试数。
- 取消令牌贯穿模型、压缩和 Tool 执行。只读且无外部承诺的 Tool 可 interrupt；可能跨过外部副作用或持久提交边界的 Tool 必须返回 authoritative settlement。
- 已发送但无法确认 Provider/Core Server 是否执行成功的动作必须报告不确定终态，禁止自动重放。
- assistant 终态持久化有短暂、有限重试；如果仍失败，应暴露持久化错误，不重新调用模型或 Tool。
- Managed Playwright 的 Tool timeout 由 Main 拥有，显式 BrowserRisk 人工等待会暂停但不重置预算；Rust reverse `CallTool` 仍受 cancellation/shutdown，而不叠加会抢先到期的 transport deadline。

## 不变量

1. Runtime 内部统一使用 Provider-neutral 消息与 Tool 闭环；Provider wire 格式不能成为历史真源。
2. 权限、审批和 Tool 可用性由 Core Server 决定，模型参数不构成授权。
3. 同一 Tool Call ID 只能得到一个权威结算；恢复必须幂等。
4. Tool 调用与结果在长期 Trace 中不可拆分；活动 Trace 只允许按 Trace 契约暂存可恢复的尾部 open call。
5. Provider 私有 continuation 不进入公开 Trace、Renderer 或普通历史 archive。
6. 模型响应已返回后，诊断/观测写入失败不能触发模型重放。
7. frozen toolset、Provider policy 与 checkpoint identity 必须共同校验，不能只比较 Tool 名。
8. Automation Run 的配置与权限在入队/admission 边界冻结；普通 UI 设置变化不得重写已开始的 Turn。
9. pending assistant content 为空且不属于模型历史；任何 UI thinking 状态只能是派生展示。
10. FileChange successor Observation 与 Run grant ref 仅在私有恢复边界有效，不能从公共 Trace/Renderer 重建。
11. 人工审批 ticket 不随短期 payload TTL 自动结算；材料缺失的晚批准只能产生 definitely-not-dispatched 失败。
12. Schema、能力指南与 World State 使用同一请求快照；稳定前缀优化不能冻结过期权限或引入额外模型调用。
13. 请求重排不改原 journal、消息角色、工具配对与 continuation；`request_order` 只能维持存活内容的布局次序。

## 代码真源

- Runtime：`crates/core/src/runtime.rs`、`crates/core/src/runtime/`
- 上下文与压缩：`crates/core/src/context/`、`crates/core/src/context_compaction*.rs`
- Provider 请求布局：`crates/core/src/context/frame/request_layout.rs`、`frame/measurement_state.rs`
- 动态能力快照与指南：`crates/core/src/runtime/extensions/`、`runtime/driver.rs`
- Provider 注册：`crates/core/src/provider_registration.rs`、`provider_profile.rs`
- Adapter 与 transport：`crates/core/src/llm/`
- continuation vault：`crates/core/src/provider_continuation_store.rs`
- Core Server 生命周期：`crates/core-server/src/application/agent.rs`、`application/agent/`
- Provider 切换：`crates/core-server/src/application/agent/provider_transition.rs`
- 工具集冻结：`crates/core/src/tools/tool_set.rs`
- Automation HumanRoot Turn：`crates/core-server/src/application/agent/automation_turn.rs`
- FileChange checkpoint/恢复：`crates/core/src/runtime/checkpoint.rs`、`crates/core-server/src/application/agent/pending_action_store.rs`

## 测试

- `crates/core/tests/provider_profile_contract.rs`
- `crates/core/src/llm/tests/deepseek_runtime.rs`
- `crates/core/src/runtime/tests/`
- `crates/core/src/runtime/tests/request_layout.rs`
- `crates/core/src/runtime/checkpoint/tests/`
- `crates/core-server/src/application/agent/tests/provider_profiles.rs`
- `crates/core-server/src/application/agent/tests/provider_runtime_capability_boundary.rs`
- `crates/core-server/src/application/agent/tests/provider_transition.rs`
- `crates/core-server/src/application/agent/tests/pending_actions.rs`
- `crates/core-server/src/application/agent/tests/automation_turn.rs`
- `crates/core-server/src/application/agent/tests/file_change_permissions.rs`
- `crates/core-server/src/application/agent/tests/mcp_approval_expiry.rs`

建议最小验证：

```bash
cargo test -p mycopilot-core --test provider_profile_contract
cargo test -p mycopilot-core deepseek
cargo test -p mycopilot-core runtime
cargo test -p mycopilot-core-server provider
```

具体 package 名与筛选项以 workspace `Cargo.toml` 和 `cargo test --workspace --no-run` 为准；不要把上述便捷命令当成 CI 真源。

## 变更检查表

- [ ] Provider 注册、UI descriptor、Adapter registry 和 contract fixture 同步更新。
- [ ] 明确新 Provider 的工具交换、Usage、stream、partial trace、continuation 和 checkpoint 能力。
- [ ] 新重试条件证明“请求尚可安全重放”，并覆盖 cancellation/timeout。
- [ ] Checkpoint schema 变更提供版本校验、旧数据失败方式和恢复测试。
- [ ] FileChange Observation/run-grant ref 只进私有 checkpoint，恢复时由 durable Host authority 重新验证。
- [ ] 人工 ticket 与 execution material TTL 分开测试，材料缺失的晚批准不会 dispatch。
- [ ] Toolset 或 Capability 变更覆盖暂停后恢复与同 Turn 激活。
- [ ] continuation 变更覆盖加密、promotion、release、fork、rewrite、delete 和 Provider 切换。
- [ ] 终态持久化失败不会重复产生模型调用或 Core Server-owned 副作用。
- [ ] 同步上下文、Trace、Tool 消费者和存储文档中的受影响边界。

## 当前限制

- 当前仅注册上表三类 Provider；兼容端点仍必须满足对应 Adapter 的 wire 契约。
- Provider-native continuation 不是跨 Provider 的通用格式；切换模型可能要求显式适配。
- Runtime 仍有固定的 Tool 迭代与压缩尝试保险上限，不保证任意长度任务永不终止。
- 进程崩溃后只能恢复已进入持久边界的状态；纯内存中且未提交的模型流片段会丢失。
- 诊断和测试大量依赖 SQLite 开发 schema；非当前 schema 的开发数据库不会原地升级。
- 人工审批没有通用自动超时；ticket 可长期 pending，而短生命周期执行材料可能在用户决定前失效。

忽略非阻塞问题现在产生一次普通后端历史事件，下一次自然请求获知；它不再是 RequestOnly 尾部状态。运行结束后的事件使用通用 after-message 位置，后续与普通历史一起压缩。不会新建 User、guidance、Wake 或额外模型请求。
