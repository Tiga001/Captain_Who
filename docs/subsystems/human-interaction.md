---
status: current
audience: developers
owner: engineering
last_verified: 2026-09-14
---

# 人机交互 Tool：阻塞与非阻塞问答

本文件是 Captain Who 人机交互（Human Interaction）的当前开发契约。它说明根 Agent 如何向用户请求信息、判断、反馈或亲自参与，并可靠地接收结果。已完成的分轮实施记录不再充当当前规范，见[人机交互分轮交付历史](../archive/human-interaction-rollout-history.md)。

## 范围与术语

人机交互包含两个动态 Tool：

| Tool                       | 适用情形                   | 对当前 Run 的影响      | 回答如何进入模型                                                   |
| -------------------------- | -------------------------- | ---------------------- | ------------------------------------------------------------------ |
| `request_user_input`       | 后续工作必须等待用户参与   | 阻塞：暂停当前 Run     | 原 ToolCall 的唯一成功 ToolResult                                  |
| `request_user_input_async` | 仍有不依赖回答的工作可继续 | 非阻塞：接纳后立即返回 | 后续 UserGuidance 或新的 HumanRoot Turn，绝不成为第二个 ToolResult |

两者都以一批 `questions` 作为输入；每项可有可选的建议选项。这里的“问题”也可以是请求用户执行某个实际操作。模型只提交题目文字和选项，不能指定批次、问题、选项、Conversation、Run 或 ToolCall 的归属 ID；这些均由 Host 创建或验证。

“阻塞”只描述该 Tool 所在的逻辑 Run 必须等待用户；它不是进程阻塞，也不等于用户操作系统被锁定。“非阻塞”只允许继续不依赖答案的工作，不能把 `accepted` 误当作用户已回答、已同意或所请求操作已经发生。

## 可用条件与默认设置

人机交互是 Host 管理的动态能力，而不是每次请求都固定暴露的 Tool。当前行为如下：

- 仅直接面向用户的、活动的根 Agent Conversation 可以创建问题。所有层级的子 Agent，以及无人值守的 Scheduled Automation Run，均不能挂载或调用这两个 Tool。
- “设置 → 个性化 → 人机交互 → 允许智能体向人类发起提问与协作”默认开启。设置单独保存，使用 revision/CAS 更新，不受 Prompt Preferences 覆盖。
- 每个自然模型请求边界冻结一次策略快照；同一快照同时决定 Tool schema、专项提示、Capability 和模型可见的 World State。设置读取失败、用户关闭开关或缺少可信执行链路时，Tool 和专项提示都不暴露。
- 关闭开关只拒绝新的问题，不撤销已经接纳的批次。已有阻塞问题仍可提交；已有非阻塞问题仍可提交或忽略。
- Tool 仍不替代执行权限或审批。用户的回答、偏好、跳过或忽略均不会授予文件、命令、MCP、浏览器或其他权限。

Rust Core 的动态能力和同请求快照真源是[runtime/extensions/human_interaction.rs](../../crates/core/src/runtime/extensions/human_interaction.rs)；两个模型可见的定义在[tools/human_interaction.rs](../../crates/core/src/tools/human_interaction.rs)。

## 选择阻塞还是非阻塞

```text
Agent 发现需要用户参与
        |
        +-- 不获得回答就不能安全或有意义地继续
        |       -> request_user_input（暂停原 Run）
        |
        +-- 仍可完成独立的读取、分析或准备工作
                -> request_user_input_async（接纳后继续当前 Run）
```

典型阻塞情形是必须先知道目标分支、外部账户、优先级或最终决策，才能选择下一步。典型非阻塞情形是 Agent 可以先读取项目、整理方案或完成其他独立检查，同时询问用户的格式偏好或待确认事项。

模型提示明确要求：不要轮询、重复同一问题，且不能将用户的意向当作已经执行的事实。收到回答后，若下一步依赖可验证的外部状态，仍须使用已有且获授权的能力核验。相应提示与 Tool 描述均把人机交互和权限审批分开。

## 请求、回答与状态

每次 Tool 调用只创建一批不可变题目。用户必须为该批中的每题选择一个选项、填写非空自定义文字或标记“跳过”，然后一次提交整批。选项、文字和跳过三种答案互斥；全部跳过仍是一条正式回答。

| 请求状态    | 含义                              | 可采取的后续动作                |
| ----------- | --------------------------------- | ------------------------------- |
| `open`      | 已接纳，等待用户处理              | 完整提交；仅异步批次可忽略      |
| `submitted` | 不可变回答已保存                  | Host 投递或恢复；不能编辑或补答 |
| `ignored`   | 用户忽略了一批异步问题            | 不创建回答投递或额外 Run        |
| `cancelled` | 阻塞等待所属 Run 已停止或无法继续 | 不接受迟到提交                  |

正式提交还会生成独立的 Delivery 状态。`pending` 表示答案已保存、尚未获得安全投递路径；`bound` 表示已绑定到引导队列或新 Turn；`applied` 表示已经由目标模型请求消费；`cancelled` 或 `failed` 表示不得自动重放。请求状态和 Delivery 状态有各自的单调 revision，不能把“提交成功”误读为“模型已经继续完成”。

目前没有产品层面的用户答题超时或自动催答定时器。`open` 会持续到用户提交、异步忽略或阻塞 Run 被停止；测试中的超时只用于测试等待，不是用户可见策略。

提交与忽略都携带稳定 `submissionId` 和预期 revision。相同身份、相同内容的重试返回原有结算；相同身份配不同内容或过期 revision 会冲突。Renderer 遇到结果不确定的网络/进程错误时锁定原草稿并复用同一次提交身份，而不是构造可能重复的第二次答案；确定的冲突、输入或访问拒绝会刷新权威快照后再允许用户处理。

共享 DTO、输入边界和跨语言解析在[crates/core/src/human_interaction.rs](../../crates/core/src/human_interaction.rs)与[packages/protocol/src/humanInteraction.ts](../../packages/protocol/src/humanInteraction.ts)；持久化事务在[crates/core/src/storage/human_interaction_repository.rs](../../crates/core/src/storage/human_interaction_repository.rs)。

## 阻塞路径：暂停并恢复同一 Run

`request_user_input` 由 Runtime driver 识别为内部 suspension，而非普通 ToolResult：

```text
模型调用 request_user_input
  -> Runtime 冻结 checkpoint、已完成结果和剩余 Tool 队列
  -> Host 原子保存 open 批次、私有恢复材料和 waiting 状态
  -> 当前执行 segment 退出，Run 进入 waiting_for_user_input
  -> 用户提交整批答案
  -> Host 领取并校验恢复权
  -> 原 ToolCall 获得唯一 human_interaction_response ToolResult
  -> 原 Run 从暂停点继续，剩余 Tool 按原顺序执行
```

暂停期间该 Conversation 保留逻辑 Run 占用：不能新建同聊天 Run、切换 Provider、手动压缩或创建分支。停止该 Run 会取消阻塞请求并拒绝迟到回答。已保存但尚未安全恢复的回答会在启动时对账；一旦无法判断可能有副作用的恢复是否已执行，系统保守地失败而不盲目重跑模型或 Tool。

恢复仅接受 Host 生成的绑定和检查点。用户回答不会被投影成一条普通 `role=user` 消息；它只作为原 ToolCall 的一次结构化 ToolResult 进入模型上下文，同时可以在界面中显示为问答气泡。阻塞恢复和停止围栏的 Core Server 入口在[human_input.rs](../../crates/core-server/src/application/agent/turn_executor/human_input.rs)。

## 非阻塞路径：先接纳，后投递

`request_user_input_async` 只有在可信 Host 已原子接纳问题后才返回：

```text
模型调用 request_user_input_async
  -> Host 验证根 Agent、Run、ToolCall、实时设置和停止范围
  -> 保存 open 异步批次并通知界面
  -> 当前 ToolCall 得到 accepted/requestId
  -> 当前 Run 继续独立工作

用户以后提交整批答案
  -> 保存 immutable response + pending delivery
  -> 活跃 Run 可安全接收时：排入 UserGuidance
  -> Conversation 真正空闲时：按正常入口创建新的 HumanRoot Turn
  -> 目标模型请求消费后标记 applied
```

`accepted/requestId` 只表示问题已记录，不包含用户回答，也不证明请求的事项已经完成。异步回答绝不回填为这个 ToolCall 的第二个结果。若当前 Run 已经自然结束，已提交回答可以触发同一 Conversation 的后续 HumanRoot Turn；如果 Run 仍在安全引导边界，回答以既有 UserGuidance 队列进入。

新建空闲 Conversation Turn 前必须通过当前的执行访问检查，包括账户登录和软件许可。该检查仅适用于“仍为 `open` 的异步批次、Conversation 空闲且提交将启动新 Turn”的情况；拒绝发生在不可变回答写入前，因此该问题保持 `open`，用户在恢复登录或许可后可以重新提交。已保存的同步恢复、重放同一结果，或投递到仍被占用的 Conversation 不走这一新 Turn 的预检。

异步批次可选择“忽略全部”。忽略不会发送草稿、普通用户消息、UserGuidance 或 Wake，也不额外调用模型；系统将最小的 ignored 事实写入普通后端历史，供下一次自然采样或启动对账观察。逐题选择“跳过”再提交则仍是正式回答，不等于忽略。

创建分支时，仍为 `open` 的非阻塞批次会随分支继承：题干与选项保持冻结，owner 身份、Run、ToolCall 和请求 ID 改为分支自身，不携带任何答案或投递历史；源会话中的对应批次不受影响。已在源会话中提交或忽略的批次不会被带入分支。

提交、Stop、压缩、审批、空闲续接和启动恢复由事件驱动调度，不轮询用户。Stop 不会删除仍 `open` 的异步问题，但可以取消已接纳且尚未投递、属于被停止范围的 Delivery；已应用、失败或取消的 Delivery 不会因重启重新投递。异步接纳与投递协调在[human_input_async.rs](../../crates/core-server/src/application/agent/turn_executor/human_input_async.rs)。

## 界面、可见性与通知

Renderer 使用 Host API 的完整快照作为权威状态，`requestChanged` 与 `settingsChanged` 只用于快速失效通知；Core Server 重连、窗口重新获得焦点、网络恢复或页面重新可见时会重新查询。Renderer 不能自己创建问题、指定 Run、伪造回答证明或直接启动后续 Turn。

- 一批问题在聊天时间线中显示一个入口，而不是为每题创建入口；异步批次可以最小化后从该入口恢复。
- 展示优先级固定为审批 > 阻塞问题 > 非阻塞问题。被抢占的未提交草稿和页码保留在当前 Renderer 内存。
- 提交后，问题和答案按题序显示为一个用户外观的问答气泡；忽略的异步批次不显示正式回答气泡。
- 当前通用原生通知只覆盖任务终态和待审批等事实；人机提问本身不会额外创建系统原生通知。用户应在 Conversation 的面板或时间线入口处理问题。
- 未提交的选项、文字和页码只保存在 Renderer 内存，关闭窗口或重载页面会丢失；已接纳的问题、已提交答案及 Delivery 回执会持久化并可在同版本重启后重新读取。

前端状态与 UI 真源为[src/renderer/src/features/humanInteraction/](../../src/renderer/src/features/humanInteraction/)，跨进程通道为[src/main/ipc/humanInteractionIpc.ts](../../src/main/ipc/humanInteractionIpc.ts)和[src/preload/HumanInteractionIpcBridge.ts](../../src/preload/HumanInteractionIpcBridge.ts)。通用通知的当前事实类型见[通用通知](notifications.md)。

## 数据、隐私与安全边界

Host 在本机 SQLite 中分别保存设置、问题批次、不可变回应、投递回执、阻塞恢复材料和异步绑定。公开快照只含渲染与提交所需的题目、回答和状态；恢复 checkpoint、可信 owner、Run 领取身份和敏感恢复输入保持在 Host 私有存储，不能由模型或 Renderer 提供。

问题和答案属于 Conversation 的本地运行历史，会随相应的 Trace、历史归档、压缩和分支边界按既有规则处理。未回答的非阻塞提问会随分支继承为分支自己的新请求（题干与选项冻结、身份与 ID 换为分支自身，不含答案与投递历史）；阻塞 suspension 和可执行 delivery 不会被分支复制为新的活动权限。恢复信封采用字段白名单；API Token、带秘密的 endpoint 和不可持久化 MCP 原始参数不写入人机交互表。

人机交互内容仍可能进入用户选定模型 Provider 的上下文，或在用户指示的外部 Tool 中使用；本子系统不应被表述为独立的网络隔离机制。公开的数据边界见[数据与权限](../../public-docs/security/data-and-permissions.md)。

## 修改此能力时的检查表

修改任何 Tool 描述、输入字段、状态、设置、存储、恢复、投递或 UI 行为时，应同步检查：

1. Rust 与 TypeScript 协议、fixture 和严格解析是否一致；不得让 Renderer 输入 owner、问题 ID、Run 或恢复权。
2. 同一模型请求的 Tool、Capability、专项提示和 World State 是否来自同一策略快照；根 Agent、子 Agent 和 Automation 的边界是否仍成立。
3. 阻塞路径是否只形成原 ToolCall 的一个结果，且停止、重启、连续同步提问和审批交接不会重跑未知副作用。
4. 异步路径是否在接纳后继续当前 Run、回答只投递一次、空闲续接仍走当前账户/许可和权限入口，ignore 不会 Wake 或创建普通 User 输入，并且分支只继承仍为 `open` 的批次、不复制回答与投递。
5. UI 是否处理通知乱序、刷新、审批抢占、最小化、草稿内存性、IME/Enter/Escape、提交不确定性和账户/许可拒绝。
6. 当前文档、[公开能力说明](../../public-docs/user/capabilities/human-interaction.md)和[用户操作说明](../../public-docs/user/everyday-use/answering-questions.md)是否仍与实现相符。

建议至少运行以下定向验证；具体筛选可随测试文件演进调整：

```bash
cargo test --locked -p mycopilot-core human_interaction --lib
cargo test --locked -p mycopilot-core conversation_fork_repository --lib
cargo test --locked -p mycopilot-core-server application::agent::tests::human_input --bin core-server
cargo test --locked -p mycopilot-core-server application::agent::tests::human_input_async --bin core-server
pnpm exec vitest run --project unit packages/protocol/src/humanInteraction.test.ts src/main/core/ipc.humanInteraction.test.ts src/renderer/src/features/humanInteraction/__tests__/humanInteractionController.test.ts
pnpm test:human-interaction-core-e2e
pnpm check:docs
pnpm check:public-docs
```

关键回归测试分布在[crates/core/src/runtime/tests/human_interaction.rs](../../crates/core/src/runtime/tests/human_interaction.rs)、[crates/core-server/src/application/agent/tests/human_input.rs](../../crates/core-server/src/application/agent/tests/human_input.rs)、[crates/core-server/src/application/agent/tests/human_input_async.rs](../../crates/core-server/src/application/agent/tests/human_input_async.rs)以及[src/renderer/src/features/humanInteraction/**tests**/](../../src/renderer/src/features/humanInteraction/__tests__)。
