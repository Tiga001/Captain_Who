---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-09-16
---

# Multi-Agent 当前架构

本文是当前 Multi-Agent 实现的规范文档。历史上的“六轮实施路线”已移至 [Multi-Agent rollout 历史](../archive/multi-agent-rollout-history.md)，不得再用轮次编号判断功能是否已实现。

一句话定义：**协作层维护持久 Agent 父子树、Mailbox、Wake、receipt 与事件；每次真正执行仍交给唯一 Agent Loop。**

## 1. 范围与不变量

1. 图中只有 Agent 节点，唯一边为 `parent_agent_id`；它是一棵树，不是通用 DAG 或 workflow DSL。
2. 每棵树由唯一根 Agent 和 `root_conversation_id` 标识。Project 只提供隔离和索引，不合并多个根 Agent。
3. 每个 Agent 唯一绑定一个持久 Conversation。Agent、Turn/Run 和内存 Runtime 是三个不同概念。
4. MCP、Skill、Command Session、Artifact、Approval、Context 与模型调用仍走现有 Agent Runtime。
5. 同一 Agent 同时最多一个活跃 Turn；根 Agent Human Turn 和子 Agent Wake Turn 共用进程级并发上限。
6. Usage 仍归各自 Conversation、assistant message、Run 和模型，不在树层重复聚合。
7. 用户只与根 Agent 交互；子 Agent Conversation 是 observer-only。模型侧的父任务可投影为 `role=user`，但持久 origin 必须是 `agent`。
8. 所有结果、状态和 cursor 先提交 SQLite，再发布通知。channel、`Notify` 和 Renderer store 不是权威状态。
9. Turn 的 completed/failed/interrupted 不会删除 Agent；后续 `followup_task` 可再次唤醒。长期生命周期另由 active/disabled/archived 表示。
10. 对未知外部副作用绝不自动重放；恢复无法证明安全时必须得到 `outcome_unknown`。
11. 同一 task tree 可复用 durable attachment、managed Artifact 与 Agent browser download，但 scope 只能由 Host 从 `agent_nodes` 的 root identity 解析；opaque locator 或调用方自报 root 不构成授权。

Scheduled Automation 与 Multi-Agent 共用 Agent Runtime 和进程级 Turn gate，但不是 Agent 树调度器。
`existing_chat` destination 只接受活跃根 Agent Conversation，不能直接绑定或唤醒子 Agent；需要协作时，
Automation 启动的根 Agent Turn 必须通过现有六个协作 Tool 创建或跟进子 Agent。详见
[Scheduled Automation](../subsystems/scheduled-automations.md)。

## 2. 运行时组成

```text
模型调用六个协作 Tool
        │
        ▼
mycopilot-core 严格 schema + AgentCollaborationExecutor port
        │ Host 注入可信 caller/run context
        ▼
Core Server AgentHarness
        │
        ▼
AgentCollaborationService / Authorizer / WaitKernel
        │                         │
        ▼                         ▼
SQLite Agent Graph            process Notify（仅加速）
Mailbox / Wake / receipts
        │
        ▼
AgentDispatcher → 统一 Turn executor → 原 Agent Runtime/Tool
```

核心职责：

- `mycopilot-core` 声明六工具、Host port、持久 Agent Graph 类型、repository、receipt 与事件。
- `AgentHarness` 从可信 `ToolExecutionContext` 构造 caller，执行授权和 application use case；模型参数不能指定 sender/根 Agent/project。
- `AgentDispatcher` 领取 Wake、持有共享并发许可、续租、启动/观察 Turn、结算结果并执行恢复。
- `AgentWaitKernel` 只等待 caller 自己可接收的持久结果、消息和 target 状态。
- `CollaborationAuthorizer` 是 Harness、Renderer RPC 和旧 Conversation 入口共同的权限边界。
- Renderer `CollaborationStore` 只维护根 Agent 范围的 tree/activity projection；子 Agent 消息仍由同一 Conversation reducer 展示。

## 3. 持久模型

### AgentNode

节点保存稳定 ID、根 Agent/父 Agent、独立 Conversation、可空 Project、树内唯一 task name/path、创建 request ID、模板/模型 snapshot、lifecycle/revision 和时间。内部 Agent ID 只供 Host 持久化、授权和 Renderer 使用；模型的协作身份和寻址只使用精确 task name。

- 根 Agent 对旧 Conversation 按需幂等物化。
- 新建或从会话分支创建的根 Agent 使用固定协作名称 `主智能体`；普通聊天标题不参与协作寻址。`主智能体` 是保留名，工具参数与后端创建入口均拒绝任意层级的子节点使用。子任务使用简短的工作名称，避免复制带引号或截断标记的整段标题。
- 子 Agent spawn 在同一事务创建/导入 Conversation、冻结 snapshot、AgentNode、initial task Mailbox、唯一投影/ack 和 queued Wake。
- task name 在整棵树内唯一且不可变，覆盖根、各层子节点以及已完成、停用和归档节点；重复名称不能创建新节点，应使用原名称继续已有任务。
- task path 由后端从父子关系生成；模型输入不接受路径或内部 ID，也不提供这些字段作为协作寻址信息。
- 模板更新只影响未来节点；模型 snapshot 冻结精确 `model_config_id` 与审计能力，不复制 Token、URL 或 Provider 凭据。
- 模板定义属于工作区级模板库；`project_agent_template_bindings` 决定一个项目未来可以 spawn 哪些模板。删除项目或取消关联只删除授权关系，不改写已创建节点的模板 snapshot。
- `fork_turns=none | all | N` 复制已结算的逻辑轮次；FileChange 可见 Staged history 与 terminal audit 按专用逻辑重映射，active 尾部、Usage、pending action、FileChange Run grant、Command Session、provider continuation 等不复制。

### MailboxMessage

Mailbox 是 Agent 间传输的唯一真相。消息保存 sender/recipient/根 Agent、kind、payload、FIFO sequence、request ID 和 delivery/lease 状态。

- 消息正文与身份一旦入队不可修改。
- 同一 recipient 同时最多 claim 一条；投影/ack 与 claim 使用 SQLite 事务和唯一约束。
- `messages.source_agent_message_id` 保证一条 Mailbox 最多一个 Conversation projection。
- 普通发送只入队，不暗示目标正在执行。

每个 recipient 的未绑定 model receipt 配额：

| 配额         | 硬上限 | 普通 message/follow-up 软上限 |
| ------------ | -----: | ----------------------------: |
| 条数         |  1,024 |                           960 |
| payload 总量 | 16 MiB |                        15 MiB |

预留的 64 条/1 MiB 用于 initial task、terminal result 等协调事实，总硬上限仍不可突破。

### WakeRequest

Wake 表示“需要一次执行机会”，不是线程或可无条件重试的副作用。

- `send_message` 不创建 Wake；`followup_task` 在同一事务写 Mailbox 与 deferred Wake。
- Wake 依 FIFO sequence 领取，持久 claim lease 为 60 秒，半开区间为 `[claimed_at, lease_expires_at)`。
- `status_revision` 只在语义状态变化时增加；单纯 owner/lease 续期不增加。
- result outbox、Wake 终态及必要的父 Agent deferred Wake 在一个事务内提交，随后才通知。

### Receipt 与事件

- `agent_model_batch_receipts` 以 `(run_id, model_batch_index)` 锁定一次模型采样 admission；Mailbox item 只能被 safe sampling 或 wait 的一方消费。
- `wait_agent` 的 target snapshot、cursor、ToolResult/model-context prefix 在同一事务预提交。Runtime 接回结果后仍须发布 Trace 与 model-context 成对快照，由 observer 幂等确认已提交前缀并同步 Host 内存；不得追加第二条 ToolResult、再次消费 Mailbox 或重复执行 wait。后续取消或失败结算必须保留这条已经成功的等待结果。
- `agent_collaboration_events` 是根 Agent 本地单调 invalidation outbox，不是第二份聊天表。snapshot/read 先取得保守 replay cursor，允许重复 replay，不允许 cursor 越过未观察状态。

## 4. 六个模型工具

工具集合必须精确为六个：

| Tool              | 当前语义                        | 关键约束                                                                    |
| ----------------- | ------------------------------- | --------------------------------------------------------------------------- |
| `spawn_agent`     | 创建直接子 Agent 并排队初始任务 | selector 精确匹配；`task_name` 256 bytes；message 64 KiB；`fork_turns` 明确 |
| `send_message`    | mailbox-only 消息               | 不创建 Wake，不保证目标执行；Tool 描述用于子 Agent 向父 Agent 汇报/求助     |
| `followup_task`   | 向严格后代分配、继续或返工      | 创建可靠执行机会，但不与目标现有 Turn 并发                                  |
| `wait_agent`      | first-ready 等待消息/结果/状态  | 1–32 个精确任务名称；默认 30 秒；0 为立即检查 ready 条件；最长 300 秒       |
| `list_agents`     | 返回授权范围内简洁树投影        | 只读，不泄漏内部 lease/checkpoint                                           |
| `interrupt_agent` | 中断严格后代当前任务            | 不删除节点、Conversation 或历史；回执与实际终态分离且可幂等恢复             |

不存在 `wait_any` 或第七个协作工具。六个实现由 Runtime extension 注册，以 `agent.collaboration` 动态 capability 整组暴露；它们不属于配置稳定的 Tool 前缀。

Harness 与 `spawn_agent` 描述包含委派时机指引：任务能自然拆成相互独立、边界清晰、交付物明确的子任务时（并行调查、独立复核、只读审查），优先创建子 Agent 并行推进；强依赖或必须串行的步骤不硬拆，单步或很小的工作也不为形式而拆。拆分后由父 Agent 负责分派、跟进与交叉核对，最终结果不能只是简单拼接。

设置的子 Agent 页面提供全局能力开关，默认开启。每个根 Turn 在原子 admission 中冻结设置，Spawn、Followup 和子任务结果所产生的 Wake 在同一事务内继承来源 Run 的策略。已启动的任务树完成本轮协作；后续新根 Turn 使用更新后的设置。`agent_collaboration_run_policies` 和 `agent_collaboration_wake_policies` 是不可改写的 Host 记录，随所属历史删除，Fork 不复制其执行授权。

同一策略同时决定六个 Schema、完整协作规则与可用模型/模板目录是否进入请求；规则与目录由扩展以 RequestOnly 方式贡献。Conversation World State 的 `agent.collaboration` 始终记录 enabled、available 和不可用原因，关闭时也保留明确状态。审批恢复验证 Host 的运行策略与 checkpoint 一致，不重新采用全局开关。空闲圆环预览新一轮策略，运行中预览使用本轮策略；其 Schema、提示词和状态计量与真实请求及自动压缩报告共用 Rust Core 投影。

Host 服务装配也遵守本轮冻结策略：关闭时不创建协作执行服务或冻结模型/模板目录，但保留关闭策略与 `disabled_by_user` 状态，以及人机交互和消息投递所需的后台根身份。Rust Core 在运行与预览入口使用同一次策略快照筛选服务，所有工具共用的检查点路径（人工审批、文件/MCP 自动执行前冻结、同步人机交互等待）只记录实际启用的协作授权。恢复时仍严格校验工具集合、协作授权和 Host 冻结策略一致，不能通过关闭开关绕过校验，也不能影响其他工具正常执行。

`spawn_agent.task_name` 是模型为新任务指定的唯一名称，其余工具的 `target`/`targets` 只接受此名称的精确值。模型从 `spawn_agent` 或 `list_agents` 的 `taskName` 复制名称，不使用 UUID、完整路径、别名或模糊匹配。Host 在可信 caller 所属树内解析名称，再使用内部 Agent ID 执行原有权限校验；名称解析不扩大同树或严格后代的权限边界。

普通工具回执使用 `taskName`，`list_agents` 额外提供 `parentTaskName`、`status`、模型显示名和最近活动时间。Mailbox 和 wait 的模型投影也只保留任务名称及必要语义；终态结果不暴露 sender/root/parent/source Agent ID、task path、Wake/Run/receipt 等内部绑定。持久记录仍保留这些身份供审计与恢复，`conversation_history` 打开 Mailbox record 时在分页前省略 Host 身份元数据，不修改自由文本或持久原文。

子结果触发新 Wake、历史目录/搜索/正文、上下文重建与摘要生成，都在读取模型视图时按可信消息来源投影 typed Result。Mailbox 与其 Conversation 原文保持数据库要求的字节一致，摘要提交仍验证原始前缀 revision。历史快照使用冻结的发送者与 Host result receipt 验证结果，不依赖源树仍然存在；普通用户或 Agent 文本不按 JSON 外形或 ID 字样替换。

主、子 Agent 共用状态汇报规则：向用户或父 Agent 说明子 Agent 的当前状态前，先成功调用一次 `list_agents`，紧接着的汇报以该次快照为准。创建/跟进的入队回执和旧消息不代表当前运行状态；查询失败只能说明尚未确认并标注最后已知情况。等待推进使用 `wait_agent`，不反复 list 轮询。`latest_completed` 表示最近一次运行已结束，任务是否成功还须核对结果与产物。这是模型行为契约，不是阻止 final 的调度门禁。

### Selector 目录

Host 给每个 Turn 冻结当前项目已关联的脱敏模板/模型目录：每类最多 32 项，总编码后 JSON 最多 16 KiB，并包含后端权威 `imageInput` 能力。模板 selector 的 Host 授权还冻结精确 template ID/revision；模型仍只提交 `agent_type`。目录只是精确 allowlist；过期、被禁用、被解绑、被修改或截断的 selector 必须 fail closed，不按名称猜测。Approval continuation 使用原 checkpoint 的目录，不在恢复时扩大权限。

## 5. 授权与配额

- 默认树深度 8、每树 64 节点、Harness message 64 KiB、全局 Turn 并发 50。
- spawn 配额在同一 `BEGIN IMMEDIATE` 内核验；幂等 request 先返回原事实，不重复占配额。
- `send_message` 的后端授权当前只校验消息大小、调用者 active、目标存在且同树；Tool 描述约定用于子 Agent → 父 Agent 报告，但 Authorizer 尚未强制方向，甚至同树其他目标也可通过。它不是方向性授权边界；工作指派必须使用 `followup_task`。
- follow-up、wait、interrupt 只允许 caller 的严格后代。名称只在 caller 当前树内解析；其他树中的同名节点不参与查找。解析后继续按内部身份校验 active、同树和后代边界，拒绝越权目标。
- 子 Agent 新 Turn 的权限以直接父 Agent 的最新持久 effective snapshot 为基线，再与完整祖先链取交集；任一 snapshot 缺失或损坏即 fail closed。
- 已开始 Run 冻结权限；Approval continuation 使用 checkpoint 原权限。下一次 Wake 才读取新的收紧策略。
- 根 Agent 可交互，子 Agent 只能通过精确的根 Agent Conversation + 子 Agent Conversation observer RPC 读取。旧历史、搜索、meta、start/steer/cancel/fork/Provider transition 和 Approval 等写入口均受同一根 Agent guard。

### 树内私有资源

`agent_tree_resource_scope` 从当前 Conversation 对应的 `agent_nodes` 行读取不可变
`root_agent_id + root_conversation_id`。这个 Host-private identity 使同一树的 root、child 和 sibling 可以
双向使用 durable attachment、managed Artifact 和 Agent browser download，也适用于没有 Project 的树；
节点完成或归档不会使其已发布结果立刻失效。

普通 Conversation、另一棵根树和猜测的 locator 继续隔离。attachment library 仍可额外按 Project
共享；browser download 也可按 Project 授权，但两者都不能用模型传入的 root ID 绕过 repository JOIN。
未知 `scheme:`/`@namespace` 被统一 locator 拒绝；确需访问同名本地文件时用显式 `./...` 消歧。

## 6. Dispatcher、等待与恢复

### Dispatcher

生产默认值：

| 参数                  | 默认值 | 代码真源                                  |
| --------------------- | -----: | ----------------------------------------- |
| 进程级 Turn 并发      |     50 | `DEFAULT_AGENT_GLOBAL_CONCURRENCY`        |
| Wake lease            |  60 秒 | Graph repository `WAKE_LEASE_DURATION_MS` |
| lease 续租间隔        |  20 秒 | `DEFAULT_WAKE_LEASE_RENEW_INTERVAL`       |
| durable fallback scan |   1 秒 | `DEFAULT_DISPATCH_IDLE_POLL_INTERVAL`     |
| 单阶段关停 grace      |   5 秒 | `DEFAULT_DISPATCH_SHUTDOWN_GRACE`         |

Dispatcher 先取得共享 gate reservation，再 claim SQLite Wake；无工作时不会伪装为活跃 Turn。交互式根
Agent Human Turn、Automation HumanRoot Turn、子 Agent Wake 和重启恢复都计入同一 gate；Automation
自身另有 2 个 Run 的并发上限。

关停算法最多可使用两个 5 秒阶段：先等待 manager 自行完成；第一次超时后请求取消非 Approval Run，并在第二个 deadline 前反复扫描 Approval 恢复竞态。因此不能把表中的 5 秒理解为完整 shutdown 的绝对上限。Main 当前 6 秒 Core Server watchdog 更短，超时后的未收口事实必须依赖 SQLite 启动恢复。

恢复规则：

1. 过期 `claimed` 且尚未 Turn admission：安全退回 queued 或确定失败。
2. `running/waiting_for_approval`：换 owner/lease 后根据 exact run、trace、checkpoint 与 pending action 分类。
3. 已有 terminal trace：观察并完成 outbox/释放 owner，不重跑 Runtime。
4. 有可恢复 Approval：继续等待或走原 checkpoint continuation。
5. admission 后无可恢复 checkpoint 且副作用可能发生：写 `outcome_unknown`，绝不盲重放。
6. recovery 不是只在启动时扫描一次；周期扫描覆盖“新进程启动时旧 lease 尚未过期”的窗口。

### 取消与显式中断

Renderer/Preload 的 `cancelRun(runId)` 契约不变。Host 将它解释为对**当前精确根 Run**的树级停止：先从受信
Conversation 解析并授权 root identity，再在 SQLite 中写入 durable tree fence。只有请求时仍处于
`in_progress` 的那个根 Run 可以建立 fence；迟到的旧 `runId` 不得停止同一根 Agent 后续显式启动的新 Turn。

fence 在同一事务冻结根 Run 以及当时已接纳的活跃后代 Run。它不是对 Agent 节点的永久禁用，也不会在之后扩张
成员范围；停止完成后，用户或模型仍可通过新的显式 Turn / `followup_task` 复用原节点。冻结成员的
`spawn_agent`、`send_message`、`followup_task` 以及 pending action/Approval continuation dispatch 都必须以
caller 的 exact run identity 查询 fence 并 fail closed，避免停止与新工作入队或外部副作用 dispatch 竞态。

被 fence 覆盖的子 Run 无论最终由取消观察器还是终态竞态结算，都不得创建 deferred parent Wake。terminal result
和 Mailbox 事实仍可持久化供 `wait_agent`、observer 与审计读取，但不能因为子节点晚到结算而重新唤醒已经停止的
父 Run。

`interrupt_agent` 是另一条、只面向一个严格后代当前执行的显式控制路径。返回
`interrupt_requested` 只表示 Host 已命中该 exact run 的运行时取消入口，不表示 Wake/Trace 已经完成
`interrupted` 结算；调用方和 UI 必须继续从持久状态观察终态。receipt 在运行时投递成功后写入
`dispatched_at`：启动恢复只重投没有该标记的 receipt，已有标记的请求只观察/补齐 durable settlement，避免把同一
中断当成新的业务动作。Agent 节点、Conversation 和历史均不因中断删除，后续新任务仍可显式唤醒它。

### Wait

`AgentWaitKernel` 执行：检查停止信号 → 查 SQLite → 注册一次性通知 → 再检查 → 再查 SQLite。之后用 50ms durable poll 保证跨进程提交或通知丢失仍可见。

- steer 优先终止当前 wait，结果保持 pending。
- timeout/cancel/shutdown 只结束本次等待，不取消 target 或删除结果。
- Agent wait 与 Command Session wait 是两个独立等待域，不能互相唤醒或消费。
- 多 target 按全局 Mailbox sequence 取 first-ready；单批最多 64 条并受有界投影预算约束，超预算内容留待后续 wait。

## 7. Approval 与 Renderer 事件

子 Agent Approval 仍使用原 `agent_pending_actions`、checkpoint 和 continuation 状态机。根 Agent UI 看到的是 JOIN 得到的投影，决定请求只提交 `root_conversation_id + approval_id + decision`；Core Server 反查 source Agent/Run/action。重复、过期或已结算决定不会启动第二次 continuation。

UI 可以在主 Agent 等待审批时更新子 Agent 状态，此时主模型已暂停，不能调用 `list_agents`。审批恢复后若要汇报子 Agent 状态，必须重新查询；自动 Mailbox 投递不等于完整最新状态快照，也不把每次 UI 状态变更自动转成模型消息。

Renderer 在根对话输入框位置聚合主、子 Agent 待审批，每次展示一张审批卡片。多条审批通过卡片上方右侧的箭头和位置/总数切换，左侧显示当前来源名称和子 Agent 头像；主 Agent 无头像，单条子 Agent 审批仍保留来源行、`1 / 1` 和禁用的切换箭头，单条主 Agent 审批隐藏该行。每条审批独立处理，Host 确认受理或结算后移出集合并更新总数；新请求不抢占当前选择，切换保留各条拒绝草稿和提交状态。审批全部清空后才恢复人机交互问题或普通输入框。展示聚合不合并主、子审批的权限 API，observer 对话仍为只读。

当前协作 notification 名必须是：

- `agent.collaboration.event`
- `agent.collaboration.observerEvent`
- `agent.collaboration.resync`

notification 只是失效信号。Renderer 通过 tree snapshot 与 `agent.collaboration.listEvents` 验证根 Agent 本地连续 sequence；重复、乱序、缺口、Core Server 重启或窗口 reload 都从数据库 rehydrate/replay。

根聊天的 semantic activity 只来自后端持久 mutation：started、updated、waiting_approval、completed、failed、interrupted。Tool 名、模型文案、时间戳或 Mailbox JSON 不得被 UI 用来反推状态。observer live event 是低延迟 overlay，durable Conversation 与 event log 才是恢复真相。

父 Agent 开始 final-response stream 时，Core Server 记录该 root 的 collaboration event sequence cut；只有不晚于 cut、且带 exact parent assistant message/Trace anchor 的活动会写入终态 message 的 `collaborationTimelineActivities`（内部最多 2,048 条）。stream reset 会丢弃旧 cut；非 final stream 或中断结算不伪造 cut。之后发生的 child activity 仍持久化并出现在 Agent Center/event replay，但不能回填已经提交的父回复，避免终态 Timeline 随后台事件变化。

## 8. Schema

当前 canonical storage 是 **v49**。唯一真源：

```rust
pub const STORAGE_SCHEMA_VERSION: i32 = 49;
```

当前 Runtime checkpoint 为 **v19**，拒绝旧版本 checkpoint；v19 使用源文件夹的 World State 模型 patch 投影，旧检查点中的整体替换文本不做兼容转换。此前模型协作身份变更也不转换含旧 Agent ID 的聊天、上下文或 checkpoint。

v49 允许纯附件引导；v48 新增历史搜索身份索引；空库原子创建 v49，exact v47 依次升级到 v48、v49 且保留全部历史，失败完整回滚；当前 v49 仍要求 exact catalog fingerprint 和外键校验。v47 新增本机 Token 统计。v46 为 Run/Wake 持久化冻结工作区并集，v45 已引入多文件夹项目（`project_folders`）。v46 及更早版本、catalog fingerprint 不匹配、非空未版本化库或外键违规均返回 `development_storage_schema_reset_required`，不修改源库，再由显式 `storage:reset-dev` 保留配置后重建。历史文档中的 v7/v8/v10/v11/v17/v19/v20/v22/v23/v24/v25/v26 只是 rollout 阶段标签，不是当前兼容声明；release runner 的 storage step 标为 canonical v49。

## 9. 代码真源

- Tool/Host 契约：`crates/core/src/agent_collaboration_harness.rs`、`crates/core/src/tools/agent_collaboration.rs`
- Graph、配额与 durable tree fence：`crates/core/src/agent_graph.rs`、`crates/core/src/storage/agent_graph_repository.rs`、`crates/core/src/storage/agent_graph_repository/tree_cancellation.rs`
- canonical schema：`crates/core/src/storage/canonical_schema.sql`、`migrations.rs`
- application service/Harness：`crates/core-server/src/application/agent_collaboration.rs`、`agent_harness.rs`
- Dispatcher/Wait/Host 取消：`crates/core-server/src/application/agent_dispatcher.rs`、`agent_wait.rs`、`agent/run_lifecycle.rs`
- 授权：`crates/core-server/src/application/collaboration_authorization.rs`
- RPC/DTO：`crates/protocol-rs/src/agent_collaboration.rs`、`packages/protocol/src/agentCollaboration.ts`
- Renderer：`src/renderer/src/features/agentCollaboration` 与 `ConversationSurface`
- Tree resource scope：`crates/core/src/storage/agent_tree_resource_scope.rs`、`attachment_repository.rs`、`managed_artifact_repository.rs`、`storage/service/browser_downloads.rs`
- Terminal Timeline cut：`crates/core-server/src/application/agent/turn_executor.rs`、`crates/core/src/storage/agent_collaboration_event_repository.rs`、`chat_repository.rs`

## 10. 测试

```bash
pnpm test:multi-agent-release
cargo test -p mycopilot-core --lib storage::migrations::tests
cargo test -p mycopilot-core-server application::agent_dispatcher
cargo test -p mycopilot-core-server application::agent::tests::collaboration_harness
cargo test -p mycopilot-protocol-rs
cargo test -p mycopilot-core storage::conversation_fork_repository
pnpm exec vitest run --project unit src/main/core/coreServer.collaboration.test.ts
pnpm exec vitest run --project browser src/renderer/src/features/agentCollaboration
```

发布 profile/smoke 的精确步骤和未覆盖项见 [Multi-Agent 发布门禁](../operations/multi-agent-release-gate.md)。恢复类变更还必须运行 queued 子 Agent restart、lease expiry、Approval continuation、Command/Agent 双等待和事件 gap/resync 测试。

## 11. 当前限制

- 不支持通用 DAG、条件边、图形工作流、自动规划器或把 MCP/Tool/Skill 做成节点。
- 不支持把 Automation destination 直接设为子 Agent Conversation。
- 用户不能直接编辑或启动子 Agent Conversation；observer 是只读投影。
- Agent-bound tree 的完整归档/物理删除需要专用生命周期事务，不能借旧单 Conversation 删除入口实现。
- 并发 50、树深 8、节点 64 是产品默认硬边界，不是生产容量承诺。
- 内存 notification 不支持跨进程广播；正确性依赖 SQLite polling/event replay。
- `outcome_unknown` 需要用户或维护者理解外部系统状态，当前没有通用自动补偿引擎。
- `send_message` 的子 Agent → 父 Agent 方向目前只写在 Tool 描述中，后端 Authorizer 仅强制同树；需要把方向作为安全不变量时必须先补实现与负向测试。
- 树内资源共享当前按整棵不可变 root identity 授权，没有成员级 grant、跨树转授权或对单个已发布 Artifact 的即时树内撤销。
- 已冻结 parent Timeline 不追踪 final-response stream 开始后的 child activity；用户需在 Agent Center 查看后续真实状态。
- Main 的 6 秒 shutdown watchdog 短于 Dispatcher 两阶段理论上限；超时退出依赖 SQLite 恢复，尚未形成完全对齐的优雅关停预算。

## 12. 变更检查表

- [ ] 工具集合是否仍精确为六个，schema/Host adapter/测试是否同步？
- [ ] 是否保持 Agent tree 而非引入隐式 DAG 或第二套 Runtime？
- [ ] 新写路径是否在一个事务内提交 Mailbox/Wake/projection/receipt/outbox 的必要组合？
- [ ] 幂等 request ID、FIFO、lease 半开区间和 per-Agent 单 Turn 是否仍由数据库约束？
- [ ] 新副作用是否定义 pre-dispatch、possibly-dispatched 和 `outcome_unknown`？
- [ ] 根 Agent/子 Agent/project/祖先权限是否从持久事实解析，而不是调用参数？
- [ ] wait 是否保持 SQLite 权威、first-ready、独立停止域和 precommitted ToolResult？
- [ ] 新 UI 状态是否来自持久 semantic event，而不是模型文本或时间戳？
- [ ] 新 tree-shared 资源是否只从 Host-resolved root identity 授权，并覆盖 root/child/sibling 与跨树/普通 Conversation 负向测试？
- [ ] terminal parent Timeline 是否在 final stream 开始处冻结，且后续事件只留在 event log/Agent Center？
- [ ] 是否更新 schema v49 后继版本、fingerprint、迁移/reset、双语言 fixture 和 release gate？
- [ ] 是否同步更新当前文档；历史轮次只在 archive 中追加注释？
