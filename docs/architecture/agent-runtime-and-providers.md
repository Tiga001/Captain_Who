---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-23
---

# Agent Runtime 与模型 Provider

本文描述 Rust Core 中一次 Agent Run 如何被创建、暂停、恢复和结算，以及模型 Provider 如何在不改变 Agent 语义的前提下接入。本文只描述后端权威状态；上下文压缩见[上下文管理](./context-management.md)，Tool 持久投影见[Conversation Trace 与 Exact Archive](./conversation-trace-and-archive.md)，数据库边界见[SQLite 存储与数据生命周期](./storage-and-data-lifecycle.md)。

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
- `ProviderProfile`：用户可选的模型端点和设置，带 schema/revision；它不是运行时能力判定的真源。
- `EffectiveToolSet`：在权限、运行时可用性、Skill/Capability 激活后冻结的本次请求 Tool 契约。
- `AgentRunCheckpoint`：审批或恢复边界保存的安全状态。Checkpoint 的 schema 是版本化协议，不能直接序列化任意运行内存。

## 当前 Provider

注册表当前包含三类 profile：

| Profile key                  | Adapter                    | 主要 wire 语义                                 |
| ---------------------------- | -------------------------- | ---------------------------------------------- |
| `generic_openai_chat`        | Generic OpenAI Chat        | OpenAI-compatible messages/tool calls          |
| `generic_anthropic_messages` | Generic Anthropic Messages | Anthropic-compatible content blocks/tool use   |
| `deepseek_v4_chat`           | DeepSeek V4 Chat           | DeepSeek reasoning continuation 与成组工具交换 |

此表用于读者定位，不应成为新的注册真源。新增或删除 Provider 必须以 `provider_registration.rs` 和 provider contract test 为准，并同步本文。

Provider 能力通过显式枚举描述，包括：Tool 交换方式、私有 replay、上下文投影、Usage 口径、partial Trace、Tool Call 来源、终态批次、Checkpoint 私有参数以及 continuation 要求。Runtime 应查询 `ProviderTurnRuntimePolicy`，不得按 profile 名称散落条件分支。

## 一次 Run 的主流程

```text
用户请求或 AutomationScheduler 预留 Conversation Turn
  -> 读取会话、模型、权限和可用服务
  -> 构造 Runtime input 与 Provider-neutral 历史
  -> 恢复可选 Checkpoint / Provider continuation
  -> 冻结 EffectiveToolSet 与 World State epoch
  -> 组装并计量 ContextFrame
  -> 必要时压缩并从 SQLite 重建上下文
  -> 发送模型请求
      -> narration / final text
      -> 一批 Tool Calls
  -> Tool 调用分为
      -> Runtime 可直接执行
      -> 需要 Core Server 执行或用户审批
  -> 每个闭环生成 Model/Event/Trace/Archive/Checkpoint 投影
  -> 继续下一次模型请求，或暂停为 pending action
  -> 原子提交 assistant message、Trace 终态、Usage/UI 终态
  -> 释放 Turn reservation
```

模型返回一批 Tool 调用时，Runtime 保留调用顺序与 Provider 要求的批次语义。Tool 调用不能在未形成权威结果时被当作已完成；需要审批的调用在 Checkpoint 中冻结，批准后由 Core Server 从持久状态恢复，而不是重新让模型生成一次。

## Checkpoint 与恢复

Checkpoint 至少绑定以下事实：

- conversation、Run、assistant Turn 和模型身份；
- 已闭合的 Provider-neutral 历史以及尚待处理的调用批次；
- 冻结 Tool 定义、Tool 身份、能力与权限相关投影；
- World State epoch、扩展快照及可恢复的 Provider continuation 引用；
- 审批动作所需的安全投影，而不是任意原始 secret 或不可信参数。

恢复时必须重新验证版本、归属、调用 ID、Tool 身份、权限和 Provider 能力。外部 MCP Server 调用与未知 Tool 不能仅凭普通 Checkpoint 保存其原始参数；它们需要各自的授权信封或明确失败。Tool 定义在暂停后变化时，应遵循冻结契约或返回结构化恢复错误，不能静默按新定义执行旧调用。

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

## 不变量

1. Runtime 内部统一使用 Provider-neutral 消息与 Tool 闭环；Provider wire 格式不能成为历史真源。
2. 权限、审批和 Tool 可用性由 Core Server 决定，模型参数不构成授权。
3. 同一 Tool Call ID 只能得到一个权威结算；恢复必须幂等。
4. Tool 调用与结果在长期 Trace 中不可拆分；活动 Trace 只允许按 Trace 契约暂存可恢复的尾部 open call。
5. Provider 私有 continuation 不进入公开 Trace、Renderer 或普通历史 archive。
6. 模型响应已返回后，诊断/观测写入失败不能触发模型重放。
7. frozen toolset、Provider policy 与 checkpoint identity 必须共同校验，不能只比较 Tool 名。
8. Automation Run 的配置与权限在入队/admission 边界冻结；普通 UI 设置变化不得重写已开始的 Turn。

## 代码真源

- Runtime：`crates/core/src/runtime.rs`、`crates/core/src/runtime/`
- 上下文与压缩：`crates/core/src/context/`、`crates/core/src/context_compaction*.rs`
- Provider 注册：`crates/core/src/provider_registration.rs`、`provider_profile.rs`
- Adapter 与 transport：`crates/core/src/llm/`
- continuation vault：`crates/core/src/provider_continuation_store.rs`
- Core Server 生命周期：`crates/core-server/src/application/agent.rs`、`application/agent/`
- Provider 切换：`crates/core-server/src/application/agent/provider_transition.rs`
- 工具集冻结：`crates/core/src/tools/tool_set.rs`
- Automation HumanRoot Turn：`crates/core-server/src/application/agent/automation_turn.rs`

## 测试

- `crates/core/tests/provider_profile_contract.rs`
- `crates/core/src/llm/tests/deepseek_runtime.rs`
- `crates/core/src/runtime/tests/`
- `crates/core/src/runtime/checkpoint/tests/`
- `crates/core-server/src/application/agent/tests/provider_profiles.rs`
- `crates/core-server/src/application/agent/tests/provider_runtime_capability_boundary.rs`
- `crates/core-server/src/application/agent/tests/provider_transition.rs`
- `crates/core-server/src/application/agent/tests/pending_actions.rs`
- `crates/core-server/src/application/agent/tests/automation_turn.rs`

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
