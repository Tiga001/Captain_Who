# Multi-Agent Architecture

本文是 MyCopilot 多智能体能力的长期架构契约。当前目标不是实现通用工作流引擎，而是在
现有 Agent Loop 外增加一个小而严格的协作底座：它持久保存 Agent 身份、父子关系、消息、
唤醒和模板，并把真正的一次执行交给现有 Agent Loop。

一句话概括：**节点只有 Agent，边只有父子指挥关系，协作只通过持久 Mailbox；Agent Loop
仍然负责思考和使用工具。**

## 范围和长期不变量

1. Graph 中只有 Agent 节点，唯一的边是 `parent_agent_id` 表示的父子指挥关系。数据结构是
   一棵树，不是通用 DAG；不支持条件边、MCP 节点、Tool 节点或工作流 DSL。
2. 一棵树由唯一的根 Agent 和 `root_conversation_id` 标识。Project/workspace 只负责隔离和
   查询索引，不能把同一项目的多个根对话合并成一张运行图。
3. 每个 Agent 唯一绑定一个独立且持久的 Conversation。Agent 是跨 Turn 的协作身份；Turn
   或 Run 是一次执行；Runtime residency 是临时内存对象，三者不能互换。
4. MCP、Skill、Command Session、Artifact、模型调用和普通 Tool 继续由同一套 Agent Loop
   负责。协作层只负责身份、父子关系、Mailbox、Wake、状态和协调。
5. 同一 Agent 同时最多有一个活跃 Turn；不同 Agent 后续可以在全局限额内并行。
6. Usage 和计费继续按各自 Conversation、assistant message、Run 和模型记录，不向父 Agent
   或整棵树聚合。
7. 用户只与根 Agent 交互。子 Agent 对用户只读；审批、取消等用户动作最终由根界面承接。
   模型侧可以把父 Agent 的任务投影成 `role=user`，但持久层必须保留真实 actor/origin。
8. 结果和状态先持久化，再发布通知。内存 channel、事件或缓存只能加速观察，SQLite 才是
   协作事实来源。
9. Agent 节点生命周期与最近执行状态分开。`completed`、`failed`、`interrupted` 只描述最近
   Turn/任务；Agent 仍可被 follow-up 唤醒。只有 `archived`、`disabled` 才是节点
   可长期保存的非活跃生命周期状态；物理删除只存在于整树删除事务中，不冒充一个持久状态。
10. 前端后续复用同一聊天 Surface 的 interactive/observer 模式，不复制一套子 Agent 聊天
    页面；没有子 Agent 时，现有对话和侧栏行为保持不变。

## 术语

| 术语                    | 含义                                           | 不等同于                       |
| ----------------------- | ---------------------------------------------- | ------------------------------ |
| Agent tree              | 以一个根对话为入口的持久父子树                 | Project、通用 workflow         |
| Agent                   | 跨多轮存在、拥有独立 Conversation 的协作身份   | 一次 Turn、一个内存 Runtime    |
| Root Agent              | 与用户对话、拥有整棵树最终交互权的 Agent       | 全局唯一 Agent                 |
| Child Agent             | 由父 Agent 分配任务、只能向父树通信的 Agent    | 可被用户直接聊天的会话         |
| Turn / Run              | 一个 Agent 的一次模型与 Tool 循环              | Agent 生命周期                 |
| Mailbox message         | Agent 间不可变的协作传输事实                   | 可任意编辑的聊天消息           |
| Conversation projection | Mailbox 事实面向模型上下文和 UI 历史的唯一投影 | 第二套协作真相                 |
| Wake request            | 请求某个 Agent 在可执行时处理待办的持久事实    | 线程、Runtime 或必然重试副作用 |
| Template                | 创建子 Agent 时使用的轻量配置引用              | Provider 配置、能力市场        |
| Template snapshot       | Spawn 时冻结到 Agent 上的不可变模板/模型选择   | 对模板表的动态引用             |

## 层次和依赖方向

```text
Renderer（第 5 轮）
  只读子 Agent surface / 根 Agent 操作
                  |
                  v
Host API + transport DTO（第 4 轮）
                  |
                  v
core-server application
  协作用例、权限、调度、统一 Turn 执行器
          |                         |
          v                         v
core domain + repositories      AgentRuntimeHostServices port
canonical SQLite                   |
                                    v
                           现有 Agent Runtime / Loop
                           MCP / Skills / Tools / Command Session
```

依赖必须继续遵守 `transport -> application -> core`：

- Transport 只做协议解析、鉴权边界和 DTO 映射，不能直接写协作表。
- 协作 application service 负责事务边界、权限和用例编排。
- 领域类型和 repository 不能依赖 Electron、Renderer 或 JSON-RPC。
- 现有 Agent Runtime 不读取协作 SQLite，也不知道树、调度器或 UI。后续它只能通过一个窄的
  Host service port 请求协作动作。
- 协作调度器不得复制 Agent Loop。它只能调用第 2 轮提取出的统一 Turn 执行器。
- Graph 层不能接管 MCP、Skill、Command Session、Approval、Artifact 或 ContextAssembler 的
  既有职责。

## 持久领域模型

以下是逻辑模型，不要求所有消费者直接访问表。

### AgentNode

一个节点至少保存：稳定 ID、根 Agent/根 Conversation 身份、父 Agent、自己的 Conversation、
可空的项目隔离键、树内唯一 task path、task name、创建请求 ID、可选模板与模型 snapshot、节点生命周期、
必要 revision 和时间。

约束：

- 根节点没有 parent；非根节点必须与 parent 属于同一棵树和同一项目隔离域。
- 一个 Conversation 最多绑定一个 Agent；同一棵树内 `task_name` 和 `task_path` 都唯一。
- 同一棵树内 task path 唯一，路径由后端根据父子关系生成和校验，不能信任模型传入的完整路径。
- 根节点对已有 Conversation 按需幂等物化，不要求启动时为全部历史对话批量建节点。
- `ensure_root` 和创建子节点都接受幂等 request ID；重试返回同一事实，不能创建重复节点。
- 节点保存创建时 snapshot，模板后续更新不能暗改既有 Agent。
- 第 1 轮低层 `create_agent_node` 只允许绑定一个尚无 message、Turn 或执行事实的全新
  Conversation，且 Conversation 的 `model_id` 必须等于冻结的模型 snapshot；绑定后子
  Conversation 不能通过普通聊天/meta 保存路径换模型，根 Conversation 仍沿用现有可切模型行为。
- 上一条是低层防误绑护栏，不表示子 Agent 永久不能继承历史。第 2 轮实现 `fork_turns` 时必须新增
  collaboration-owned 的原子 spawn/fork/bind 路径，用明确 provenance 受控导入历史；不能放宽普通
  create，也不能借 interactive user 写入路径绕过子 Agent 只读边界。

### MailboxMessage

Mailbox message 是 Agent 间唯一的协调消息真相。它至少保存稳定消息 ID、幂等 request ID、
root、sender、recipient、受控消息种类、正文或安全结构化负载、树内 FIFO sequence、delivery
状态与时间。

消息一旦入队，其 sender、recipient、正文和顺序不可修改。确认、失败或过期只能追加/迁移
delivery 状态，不能重写原事实。发送方和接收方必须属于同一棵树；普通 repository 调用不能
跨树投递。

每个 recipient 同时最多 claim 一条消息；claim 带持久 lease，可续租，也可在投影前崩溃后由
新 token 原子回收。这样既不会越过前一条消息破坏 FIFO，也不会把消息永久卡在 claimed。

### WakeRequest

Wake 是“需要一次处理”的持久请求，而不是正在运行的线程。它保存目标 Agent、原因/来源消息、
幂等 request ID、FIFO sequence、独立状态、claim/lease 信息和时间。

Repository 只提供原子入队、claim 和合法 CAS 状态迁移。本轮不执行 Wake；后续调度器必须用
数据库约束保证一个 Agent 不会同时拥有两个 running Wake/Turn。进程崩溃后依据持久状态恢复，
不能依据丢失的内存 Future 猜测结果。

claim 带有持久 lease deadline，并允许同一 claim 续租。只在“已经 claim、尚未进入 running”时
可以把过期 Wake 原子退回队列并重新 claim；一旦进入 running，后续崩溃恢复必须由第 3 轮按
Turn/checkpoint 事实判定 `outcome_unknown`、恢复或终止，不能冒险并发启动第二个 Turn。

初始 task 在创建 child 的事务内完成唯一 Conversation projection、ack 和 Wake；后续 deferred
follow-up/result 则允许 source message 仍为 queued 时先与 Wake 原子入队。任何 Wake claim 都必须
在同一个事务中先按 FIFO 完成该 source 以及更早消息的唯一投影/ack，再取得执行权；因此既不会先
启动 Turn 才发现任务正文缺失，也不存在“已投影但尚未 claim”跨事务崩溃半窗。同一 source message
最多绑定一个 Wake，崩溃后按幂等 request 重试；普通 send 只入队，不隐式创建 Wake。

### Result / Outbox

子任务结果可以是受控种类的 Mailbox message，也可以有独立的结果/Outbox 事实；无论采用哪种
物理布局，都必须遵守同一规则：终态和结果先原子持久化，随后才允许通知父 Agent或 Renderer。
通知丢失可以重放，结果本身不能重复提交。

### AgentTemplate

模板只保存项目内稳定 `machine_key`、显示名称、简短描述、子 Agent instructions、现有
`model_config_id`、enabled、revision 和时间。

- `machine_key` 是未来 Harness `agent_type` 的精确引用，创建后不随显示名称改变。
- Provider、API key、Base URL、价格、上下文长度等仍只存在于现有 Model Settings。
- Spawn 时必须通过现有模型配置解析 `model_config_id`，并生成不可变 snapshot。
- 模型 snapshot 只保存不含凭据的“创建时选择与审计身份”（模型 ID、显示能力和配置 revision），
  不是可脱离 Model Settings 重放的第二份 Provider 配置。后续 Turn 仍按 snapshot 中的精确模型 ID
  走现有模型解析链；缺失、禁用或配置无效时明确 unavailable，不能从 snapshot 猜测连接或回退模型。
- 模型被删除、禁用或当前不可用时，解析明确返回 unavailable，绝不静默换模型。
- `reasoning_effort` 不是一次 Run 的临时 Provider payload patch。未指定时完全沿用所选模型在
  Model Settings 中的现有 reasoning policy；显式指定时只接受 Runtime 当前已支持、且与该模型
  已解析配置精确一致的 `high` / `max`，并把选择冻结到 Agent snapshot。后续每个 Turn 都按冻结值
  重新核对当前模型配置；不支持、被修改或不可用都明确失败，绝不忽略、降级或临时改写请求。
- 模板修改只影响未来 Agent。删除模板也不能破坏已有 snapshot。
- Spawn/Create 节点事务会核对模板 snapshot 与当时的模板 revision 内容完全一致；已经更新或
  删除的模板不能用旧 snapshot 创建新节点，并且 create 提交时模板必须仍为 enabled；禁用发生
  在 resolve 与 create 之间时本次创建明确失败，不把旧 snapshot 当 capability token。已有节点
  引用模板时，模板可以禁用但不能物理删除。
- 模板不是 Skill/MCP 套餐、角色市场或第二套模型配置系统。

## Mailbox 到 Conversation 的唯一投影

Mailbox 与 Conversation message 服务不同消费者，但不能形成双写真相：

```text
父 Agent 发送任务
       |
       v
事务写入不可变 MailboxMessage（权威）
       |
       +--> 写入/安排唯一 Conversation projection
       |       role = user（模型投影）
       |       origin = agent（真实 actor）
       |       sender_agent_id = 父 Agent
       |       source_agent_message_id = MailboxMessage.id
       |
       +--> 必要时写入 WakeRequest
       |
       v
提交后才发布内存通知
```

具体约束：

1. Conversation message 必须保存结构化 origin/actor。旧的人类消息缺少显式 origin 时按 human
   读取；不能从 UI JSON 或正文猜测 actor。
2. 每个 Mailbox message 最多对应一个 Conversation projection；
   `source_agent_message_id`（或等价外键）必须由数据库唯一约束保护。
3. Projection 的创建与 delivery/ack 要么在同一事务完成，要么使用可恢复 Outbox。任一步失败
   都不能留下已确认但没有投影的消息，也不能在恢复后生成两条聊天消息。
4. 普通 Conversation message 继续遵循既有删除、归档和 UI 状态规则；Mailbox projection
   则是不可变协调投影，只能由未来的整树清理事务连同对应事实一起处理，不能单独修改、删除、
   伪造或重新发送 Mailbox 事实。
5. ContextAssembler 仍是模型上下文唯一组装入口。它消费 Conversation projection 的既有
   role 和正文；真实 origin 用于权限、审计和 UI 标签，不另造一套子 Agent prompt journal。
6. 自动结果同样先进入持久 Mailbox/Outbox，再通知父节点。父节点不在线、正在等待 Command
   Session 或进程重启，都不会导致结果丢失。

## 状态必须分层

不同状态机不得合并成一个大枚举：

| 层               | 描述                     | 示例语义                                                          |
| ---------------- | ------------------------ | ----------------------------------------------------------------- |
| Agent lifecycle  | 节点能否继续被使用       | active、disabled、archived                                        |
| Turn/Run         | 一次执行的结果           | queued、running、waiting approval、completed、failed、interrupted |
| Wake             | 一次持久唤醒请求         | pending、claimed、running、settled/cancelled                      |
| Mailbox delivery | 一条消息的传输/消费进度  | pending、projected、delivered/acknowledged                        |
| UI display       | 从以上事实派生的简化展示 | 处理中、等待、完成、失败                                          |

映射原则：

- UI display 是投影，不能回写为领域事实。
- Turn 完成或失败不会关闭 Agent；follow-up 可以产生新的 Wake 和 Turn。
- archived/disabled Agent 不接受新 Wake，但历史 Conversation 和 Mailbox 仍可读。
- “一个 Agent 最多一个活跃 Turn”最终由第 2 轮统一 Turn 执行器把现有根 Turn 与 Wake 都接到
  同一个持久占用事实，再由原子状态与唯一约束保证；本轮的 Wake 唯一索引只防止两个协作
  Wake 同时运行，不能被误解为已经覆盖尚未接线的普通根 Turn。无论哪一轮都不能只依赖进程锁。
- 运行超时、取消和 outcome unknown 是执行事实，不得被折叠成成功或节点删除。

## Context、权限和 Usage 边界

- 子 Agent 继续使用共享的系统提示词构建和 ContextAssembler，只增加很小的协作身份 overlay：
  它是谁、父节点是谁、当前任务是什么、只能向父树报告，不能直接对用户说话。
- 创建时的 `fork_turns=none | N | all` 只决定初始历史 snapshot；创建后父子 Conversation
  分叉演进，只通过 Mailbox 通信。
- `fork_turns` 按逻辑轮次而不是 message 行数计数，并复用既有 Conversation fork 的 settled
  assistant 候选边界：assistant 非 pending、Run JSON 不活跃，若有 trace 则必须 terminal；一个轮次
  包含自上一 settled assistant 之后的输入和该回复。协作快照比普通 UI fork 更严格：每个实际选中的
  assistant 都必须有 durable trace，才能直接进入共享 ContextAssembler；缺失时明确返回 snapshot
  unavailable，不能产生“能创建但不能执行”的子 Agent，也不能猜造 trace。`Last(N)` 可以避开更老、
  未被选中的 legacy 缺 trace 轮次。当前 active 尾部及触发它的 user 整体排除；`N` 超过历史时取全部
  可用终态轮次，`N=0` 是非法输入；调用方始终可用 `none` 创建空历史子 Agent。
- `all` 物理保留全部终态历史及当时有效的 compaction summary 链；ContextAssembler 仍按
  summary + uncovered raw suffix 组装模型上下文。`N` 只复制最近 N 个 raw 轮次，绝不暗带覆盖
  更早历史的 summary；`none` 只落不可变 snapshot header。
- 创建快照复制的是只读历史事实：消息、终态 trace、精确 model-context、Exact Archive、最终
  turn diff、选中消息附件的独立文件副本，以及相关 content-addressed Artifact grant。Usage、
  agent run/UI JSON、输入草稿、pending action、active Command Session、Provider continuation /
  checkpoint、订阅与 mutable world state 均不复制。
- `child_context_snapshots` 与 snapshot message provenance 是创建事务内的不可变事实。snapshot
  message 冻结 source conversation/message 和原始 human/Agent actor，不能复用 Mailbox
  `source_agent_message_id` 唯一投影外键；孙 Agent 因而仍能区分历史人类输入与历史父 Agent 任务。
  数据库在复制当下核验 source role/content/status/time/actor，复制完成后父历史或模板变化不反向
  改写子历史。
- 可信协作身份放在共享 `AgentRunContext`，由 Host 从 Agent/Wake/Mailbox 事实构造，并随 approval
  resume 的显式 allowlist 持久化；Renderer 和模型不能提供或覆盖。普通根 Turn 的身份为 `None`，
  系统提示词逐字保持原样；子 Turn 只在同一 prompt builder 尾部追加有界身份 overlay。
- 子 Agent 的新 Turn 以直接父节点最新的持久 effective-permission snapshot 为继承基线，并与完整
  祖先链 snapshot 逐字段取交集；任一祖先快照缺失或损坏都 fail closed。因此根权限收紧会立即成为
  任意后代下一 Turn 的上界，模板、项目策略和动态策略只能继续收紧，不能提升权限。一次已开始的
  Run 冻结其 `AgentRunContext.permissions`；Approval continuation 恢复 checkpoint 中的原值，不在
  恢复时重算。下一次 follow-up/Wake 才重新读取父/祖先的最新持久快照。
- 子 Agent 产生的 Approval 仍属于原 Turn/checkpoint，但用户可操作投影必须路由到根界面，并
  标明来源；本约束在第 4、5 轮实现。
- 每个 Agent 的 Usage 沿用现有 Conversation/message/model 记录和展示。父节点收到的结果不得
  携带一份重复计费汇总，Graph 层也不维护 tree total。

## 删除、归档和外键原则

归档优先于物理删除，且不同对象的归档不能互相冒充：

- `conversations.archived_at` 继续表示现有聊天目录中的归档状态，不等同于 Agent lifecycle。
- 归档/禁用 Agent 不删除 Conversation、Mailbox、Usage 或执行历史。
- Graph repository 删除节点绝不能反向删除根 Conversation。普通子节点也不允许绕过
  application service 单独物理删除自己的 Conversation。
- 用户物理删除一个根 Conversation 时，application service 必须把整棵树视为一个删除单元：
  先确定全部子节点/子 Conversation，再在受控事务中清理协调事实和数据库记录，最后执行既有
  附件文件清理。失败必须整体回滚，不能留下隐藏的子 Conversation 或孤儿 Mailbox。
- Project 删除按其中每个根树执行同样的显式清理。跨项目外键永远不允许级联误删。
- 删除模板或模型配置不删除 Agent；既有 snapshot 仍可审计，未来 spawn/唤醒若无法解析模型则
  明确 unavailable。
- 外键负责阻止孤儿和清理树内从属事实，但不能用一条隐式级联代替根 Conversation/Project 的
  应用层删除计划。所有破坏性路径都要有事务回滚和 foreign-key-check 测试。

## Schema 和兼容性原则

协作表属于现有 `canonical_schema.sql`，必须沿用 schema version、catalog fingerprint、外键检查
和临时数据库测试机制；禁止建立影子迁移目录或运行时 `CREATE TABLE IF NOT EXISTS` 补丁。

当前仓库只支持一个严格的开发期 canonical baseline：

- 新数据库在一个事务内创建完整 schema。
- 已存在数据库的 version 或 fingerprint 不匹配时 fail closed，并提示使用已有 development
  storage reset/rebuild 路径；当前阶段不偷偷执行 `ALTER TABLE` 原地升级。
- 本轮 schema 变化同步更新 canonical version/fingerprint 和 fresh-schema fixture。测试只使用
  repository fixture、内存 SQLite 或 `tempdir`，严禁打开、复制或升级用户 live DB。
- 旧 Conversation 不批量创建根节点；第一次使用协作能力时由 `ensure_root` 幂等懒物化。
- 旧消息的逻辑兼容由读取默认值/受控重建 fixture 验证；不能把未知 actor 猜成 Agent。
- 面向稳定发布的历史版本迁移、备份恢复演练和真实旧版本 fixture 在第 6 轮形成发布门禁；在此
  之前不得绕开 canonical fingerprint 规则。

## 明确不做

- 不实现通用 DAG、通用 `edges` 表、条件路由、Graph 画布或自动规划器。
- 不把 MCP service、Skill、Tool、Command Session、Artifact 或模型调用做成图节点。
- 不复制 Agent Loop、ContextAssembler、Approval 或 Usage 系统。
- 不允许用户直接向子 Agent 发消息，不为子 Agent 新建一套可编辑聊天业务。
- 不把 `agent_run_json`、`ui_state_json`、Renderer store 或进程内数组当协作事实数据库。
- 不因通知、网络或进程重试重复执行未知结果的副作用。
- 不在模板中复制 Provider/模型详细配置，不建设角色市场或复杂能力包。

## 六轮实施路线

### 第 1 轮：架构护栏、持久化与模板后端

落地本文、Agent tree/Mailbox/Wake/Result repository、结构化 message origin、根节点懒物化、
模板 CRUD 与 snapshot 解析。本轮不运行子 Agent，不接前端。

### 第 2 轮：统一 Turn 执行器与子 Agent 上下文

从现有根 Turn 路径提取唯一的 server-side Turn 执行入口；根和子 Agent 复用同一 Loop。实现
子 Conversation 创建、`fork_turns` snapshot 和协作身份 overlay，不复制 prompt builder。正式
Host spawn 只能调用 collaboration-owned 原子用例：提交前按实际模型 ID 和当前 revision 重新解析，
随后创建/导入子 Conversation 并绑定节点；本轮低层 `create_agent_node` 不直接暴露给 Harness。
这个用例在一个 `BEGIN IMMEDIATE` 事务中提交子 Conversation、AgentNode、不可变上下文快照、
初始 task Mailbox、唯一 `role=user / origin=agent` 投影、ack 与 queued Wake；任一步失败都不留下
半个子节点。附件先复制到独立目标并在数据库失败时清理，进程崩溃后的孤儿文件扫描留给恢复轮次。
模型选择顺序固定为“显式 model > 模板 model snapshot > 父 Agent 当前模型 > 系统默认”；只有父
Conversation 本来没有模型时才允许进入默认分支，任何已经指定或继承的模型不可用都明确失败，
不能向后回退。幂等重试读取首次冻结的完整 bundle，不受模板或模型目录后续变化影响。

公开聊天入口只能构造 `HumanRoot`，在任何写入前拒绝子 Conversation；Host 内部 Wake 使用不可
反序列化的严格类型，并从 running Wake、claim lease、Mailbox projection 和 Agent node 重新解析
协作身份。两者最终进入同一个 Turn executor 和 Runtime segment。父任务只使用已有的唯一
`role=user / origin=agent` 投影，不再插入第二条 user 消息。子权限只在同一 Turn admission 事务中从
可信父/祖先权限快照解析，不取自模型参数；事务把最终权限写入该 child 的 1:1 durable snapshot，再
构造冻结的 `AgentRunContext`。Usage 仍归自己的 Conversation。

单活跃 Turn 的权威边界不是内存 Runtime。准备阶段使用 SQLite `BEGIN IMMEDIATE`，在任何全量
Conversation 写入前检查 durable active trace，并在同一事务中提交输入投影、pending assistant 和
空 `in_progress` trace；canonical partial unique index 再提供数据库约束。审批暂停保留该占用，终态
消息/trace（以及需要的 pending 状态）都提交后才释放。内存 map 只是快速拒绝和重启恢复加速器。
有活跃 Turn 时 Agent lifecycle 也不能从 active 退为 disabled/archived，避免另一 Host 在运行中
撤销执行身份。审批 continuation 已取得 Turn owner 后若在进入 Runtime 前恢复 Skills 或上下文失败，
pending、assistant、AgentRun、terminal trace/model-context 与 Usage 必须在同一事务中失败终结；CAS
冲突或任一写入失败则原样保留占用，交给重试/重启恢复，不能只改一半状态。
读取历史时同时冻结 Conversation 的单调 revision；提交前在同一个 `BEGIN IMMEDIATE` 中做 CAS。
因此即使另一 Host 已经完成并释放 active trace，基于旧历史准备的全量快照也只能失败并重新读取，
不能删掉或覆盖刚完成的 Turn。
启动后若后续 Host 预检失败，只能按 exact run/assistant identity 删除尚无 trace item 的 provisional
事实，绝不能回滚或覆盖另一 Host 的 Turn。

用户侧模型切换与压缩入口沿用同一 Graph 边界：只有 active 根 Agent 可以进入；disabled/archived
根节点和所有子 Conversation 都在任何模型改写或压缩调用前 fail closed。子节点的模型只能由后续
可信协作运行入口按冻结选择解析，observer 页面不拥有写权限。

Agent 节点的 model snapshot 冻结创建时的选择和审计事实，不永久冻结全局 Model Settings revision。
每次新 Turn 都按 snapshot 的精确 `model_config_id` 走现有解析链读取当前 enabled 配置；无关模型
配置变化不阻塞，选中模型缺失、禁用或不可用时明确失败且不回退。一次 Turn/审批 checkpoint 则继续
冻结本次实际 Provider revision。审批续跑对持久 envelope context 与 checkpoint run_context 的
collaboration identity 做 exact 双向相等校验，并在进入 Runtime 或任何副作用前从 Graph/Wake 事实
重验 active lease；序列化 envelope schema 变更必须显式升版。

### 第 3 轮：协作运行时、Mailbox 调度与双等待

实现受限并发 Dispatcher、每 Agent 单 Turn、持久 Wake 恢复、父子消息安全注入，以及
`wait_agent` 与 Command Session wait 相互独立、均不丢持久结果的语义。

#### A：Mailbox、deferred Wake 与结果 Outbox

第 3 轮不增加平行消息表。`agent_mailbox_messages` 仍是唯一协作传输真相，单调 `sequence`
定义同时间戳下的稳定 FIFO；读取不删除记录。`queued -> claimed -> acknowledged` 的 lease
协议只描述“唯一 Conversation 投影已提交”，并不等价于“模型已经消费”。投影与 ack 在同一
SQLite 事务中完成，`messages.source_agent_message_id` 的唯一约束使重启重试不能产生第二条投影。

通信应用服务固定为两种写语义：

- `send_message` 只写 queued Mailbox，不创建 Wake；
- `follow_up` 在同一事务写 queued Mailbox 与唯一 deferred Wake，但不提前投影。Dispatcher 在
  claim Wake 的同一 `BEGIN IMMEDIATE` 中按 recipient/sequence 投影并 ack 全部早于或等于 source
  的消息，再 claim Wake；运行中 Turn 则由模型批次 receipt 绑定同一 source 后把 deferred Wake
  原子变为 `satisfied`。因此“投影后崩溃、Wake 尚未 claim”和“已进当前批次又启动第二 Turn”两个
  窗口都不存在。多个 follow-up 不要求每条单独启动 Turn：第一条取得执行权，后续条目在安全采样
  边界按 FIFO 被同一 Turn 吸收并满足各自 Wake。

动态投影若遇到 `in_progress` assistant，会稳定插在该 pending assistant 之前并只把 assistant
及其后普通行右移。未 receipt-bound 的后续 Mailbox 不会提前进入历史构建。投影 helper 与模型
批次 receipt 共用这一位置规则，Tool handler 不自行拼上下文。

权限只有四条：普通 message 可同 root tree 定向；task 只允许直接父到直接子；管理 follow-up
只允许调用者到严格后代；result 只允许子到直接父。严格后代由 `parent_agent_id` 递归链判断，不用
含 `%`/`_` 通配语义的 task path 做授权。repository 做可读错误，canonical trigger 再做数据库
防伪；跨树、祖先方向、兄弟分支和根目标 follow-up 均失败。

子 Wake 终结时使用 typed `AgentTurnResultEnvelope`：冻结 schema version、child、task path、Wake、
Turn/Run、终态、有界 summary/error，以及按 artifact ID 稳定排序的 managed Artifact refs。Artifact
只能从该 child Conversation 与 exact run 的 durable grants 导出，不猜普通附件，不暴露绝对路径，
也没有 Usage 字段。Wake 终态、result Mailbox 和非根直接父的 deferred Wake 在一个事务提交；直接
父为根时只留下 pending result，绝不后台启动根模型。终态幂等重试读取首次冻结的 envelope，之后
新增 Artifact grant 不改变已提交结果。

Wake 的 `status_revision` 初始为 1，每次真实状态变化严格加一，lease-only renewal 不增加；它和
Wake sequence 是等待/observer cursor 的持久版本。`run_id` 与 `assistant_message_id` 只能成对绑定，
并且必须引用该 Agent Conversation 的 exact durable Turn。`satisfied` 是 receipt 已由当前 Turn
消费的成功协调终态，不伪装成 cancelled 或“最近任务完成”。Agent 列表状态从 lifecycle、durable
active Conversation Turn、active Wake 和最近真实 Turn 终态派生：active/approval 优先，根 Agent 的
普通 Human Turn 即使没有 Wake 也显示 running/waiting；仅在没有 active 执行时才展示最近的
completed/failed/interrupted/outcome_unknown。上述最近任务状态不会关闭持久 Agent 身份。

#### B：Dispatcher、执行权与崩溃恢复

`AgentDispatcher` 只做三件事：从 SQLite 按 Wake sequence 领取可执行机会、取得进程级许可后调用
第 2 轮唯一 Turn executor、观察 durable trace/pending-action 直至结算。它不解释 Graph、不读取
Renderer event 判断完成，也没有第二套 Agent Loop。全局领取的固定顺序是“共享进程许可 -> SQLite
per-Agent 执行权”；SQLite 查询跳过已有 active Wake、in-progress Conversation 或仍持有 Mailbox
claim 的 Agent，因此一个暂不可投影的最老 Wake 不会阻塞其他 Agent。source FIFO 投影/ack 与
`queued -> claimed` 在同一 `BEGIN IMMEDIATE` 中提交。极窄的查询后竞争仍可能让一次投影返回
`NotReady`；Dispatcher 把它当候选级暂态冲突，在有界 fallback tick 后重新扫描，而数据库损坏或
不可用仍作为 fatal storage error 暴露。这样不把正常竞争误判成进程故障，也不会无限吞掉真实故障。

领取前先在同一个 gate 中预留容量，claim 成功才把 reservation 提升为 active Turn permit；空队列探测
会占住并发容量却不会伪装成活跃 Turn。Dispatcher 的 quiescence 边界同时要求：最近一次 durable 扫描
确认队列为空、worker/running map 为空且 active Turn 为零，避免“旧 worker 刚释放、下一次空队列探测
刚预留”的瞬时归零竞态。
每次已提交的新 Wake/result 通知还会在同一锁内递增单调 generation；空扫描只有在开始与发布之间
generation 未变化时才能宣布 idle，避免旧的空扫描覆盖扫描窗口内刚提交的新工作。

根 Human Turn 与子 Wake Turn 必须使用 `AgentService` 的同一个 `AgentTurnConcurrencyGate`。Host
构造时配置唯一进程上限（默认 4），Dispatcher 正式构造器只接受这个共享 gate，不存在另建 child
semaphore 的生产入口。permit 是共享 lease：AgentService 持有一份跨 Runtime/Approval，Dispatcher
持有另一份直到 Wake 终态、result Outbox 和父 deferred Wake 全部提交；因此 terminal trace 先落库
也不会提前腾出并发槽。审批暂停仍算一个活跃逻辑 Turn并占用槽；重启时 `AgentService` 从所有 durable
in-progress traces 恢复占用，哪怕配置被调低，也只会阻塞新工作而不会丢掉既有执行。

Wake claim 与 Turn admission 分成两个明确的崩溃边界。`claimed` 尚无 Run 身份，过期后可以安全回到
`queued`；统一 executor 在同一个事务提交 pending assistant、empty in-progress trace、turn-start
delivery receipt，并把 Wake `claimed -> running`、冻结 run/assistant 身份。此后任何 prepare/start
失败都不得删除 assistant、trace 或 receipt，而由 typed settlement 把 exact Turn 失败终结并回传结果。
`claimed -> failed` 只用于确定未完成 Turn admission 的启动失败，属于合法终态迁移。

启动恢复不是一次性扫描：Host 周期性检查租约，处理“新 Host 启动时旧租约尚未过期”的窗口。过期
`claimed` 可重排；过期 `running/waiting_for_approval` 只更换 owner token/lease（语义状态未变，所以
`status_revision` 不增加），再按 durable 事实分类：terminal trace 或可恢复审批只观察 exact Turn；
admission 后没有 trace 是确定的 pre-Runtime failure；in-progress 且无可恢复 checkpoint 则写
`outcome_unknown`，绝不盲重放可能已有外部副作用的步骤。恢复 settlement 在同一事务终结遗留 trace、
Usage 投影、Wake 和 result Outbox，提交成功后才按 exact identity 释放 AgentService 的 Conversation
占用与 permit，使该持久 Agent 可以接受下一次 follow-up。

内部 interrupt 只允许 caller 的严格后代。tree-scoped request receipt 记录首次 disposition，重复
request ID 永远返回同一 Wake/Run，不能误伤队列中的下一项；queued/未 admission 的 claimed Wake
原子 cancelled，running 传播到 exact Runtime，waiting approval 则走现有 durable pending-action
cancel/finalize 路径。进入过 Turn 的取消最终以 `interrupted` 结果回直接父；节点、Conversation、历史
都保留。审批 continuation 真正重新进入 Runtime 前，必须先用同一 claim 把 Wake
`waiting_for_approval -> running` 并增加 `status_revision`，再通知 durable observer；因此关停只保留
仍有 durable pending approval 的纯等待状态，pending 已消失且 Wake 已回到 running 的 continuation
与其他 live Runtime 一样接受 interrupt。正常关停先停止新 claim，有限等待运行中工作；进入取消阶段后
在同一个有界 grace window 内持续重扫 live execution map，只对非审批 Runtime 的 exact run 请求一次
取消。因此，即使审批恰好在首次快照之后恢复，也不会从关停边界漏过；窗口结束仍未收敛的 durable
Wake/trace 留给上述恢复规则。

Mailbox 与 Wake lease 都采用半开区间 `[claimed_at, lease_expires_at)`：旧 holder 在精确 deadline
续租、ack、transition 或 settlement 一律失败；新 Host 从 `deadline <= now` 起可以回收。owner/lease
更换不增加 `status_revision`，因为它不是用户可见的语义状态变化。

#### C：唯一安全采样边界与模型批次 receipt

Runtime 只在一个位置接收动态协作输入：每次真正调用 Provider 采样之前、steer 和上一批 Tool / Command
结果已合并之后。Core 通过窄 `AgentSamplingBoundaryInbox` Host port 请求事实；它不知道 Agent tree、
SQLite 或 Dispatcher。正式 Host 为所有 graph-bound Turn 安装同一实现，包括根 Human Turn；旧的未绑定
Conversation 返回空。因此根空闲期间积累的 child result 不启动后台模型，只会在下一次用户 Turn 的首个
采样边界进入。

`agent_model_batch_receipts` 以 `(run_id, model_batch_index)` 唯一标识一次采样 admission；item 以
`message_id` 全局唯一，使自动安全边界与 wait 路径只能有一个赢家。turn-start 路径在 Turn admission
同一个 `BEGIN IMMEDIATE` 中绑定准备阶段已经实际放入模型输入的 Agent projection IDs，不另写 trace；
因此 initial task、前置 send 和 follow-up 都只出现一次。动态 safe-boundary 则在一个事务中完成 queued
claim、唯一 projection/ack、receipt item、`AgentMailboxDelivery` trace、精确 model-context log、deferred
Wake `satisfied` 和 batch close，崩溃不存在“已消费但 trace 未落库”的窗口。

Mailbox 原文保持不变。模型看到的是共享 trace 层生成的 Host 认证 JSON envelope，明确
`origin=agent`、sender/task path/kind 和 payload，绝不冒充真人。Storage 返回 raw payload，Runtime 也调用
同一个 projector；durable trace、model-context、live Context 和重启恢复逐字一致，不产生二次 envelope。
单条 envelope 最多 32 KiB，超限按 UTF-8 边界确定性截断并带 `payloadTruncated=true`；每个动态批次最多
64 条、合计最多 128 KiB，按 Mailbox sequence FIFO 选择，超预算内容保持 pending 到后续批次。

被 safe-boundary trace 承载的 raw Conversation projection 在后续模型 history 和 compaction journal 中
排除，但仍保留给 observer 审计和 FTS 的唯一用户可搜记录；trace delivery 不重复建 FTS 正文。turn-start
projection 始终是历史唯一表示，绝不因存在 receipt 而被过滤。receipt header/item/target 都是不可删、
不可改写的协调事实；batch `sampling_bound_at` 关闭后禁止补写 item/target，删除 assistant trace 也不能
级联擦除消费事实。

Mailbox 在同一真相源上设置简单背压：每个 recipient 最多保留 1024 条、16 MiB 尚未绑定 model
receipt 的事实；普通 message/follow-up 使用 960 条、15 MiB 软上限，为 initial task、terminal result
等协调事实保留 64 条、1 MiB，但总体硬上限仍不可突破。重复 request 先命中原记录，不重复占用配额；
projection/ack 不释放容量，只有不可变 receipt item 证明进入某个模型批次后才释放。

#### D：wait_agent 内核与 Command Session 双等待域

本轮只实现内部 `AgentWaitKernel`，不注册模型工具。它只查看 caller 自己 Mailbox 中来自指定 target 的
message/result/update，以及 target 的授权状态；绝不读取或 claim target 自己的 inbox。target 必须是
caller 的严格后代，祖先、兄弟和跨树均拒绝。多个 target 是 first-ready：返回本次已就绪快照，不增加
all/quorum/wait-any DSL。

等待采用“先检查控制信号 -> 查 SQLite -> 注册一次性通知 -> 再检查控制信号 -> 再查 SQLite”。同进程
commit 后的共享通知只降低延迟；50ms 有界 durable poll 使通知丢失、另一 StorageService/进程提交或
重启仍然正确。steer 优先结束当前 wait，且在任何有副作用的 poll 前检查，所以结果仍 pending；timeout
同样不取消 target 或删除结果。Command Session wait 只响应指定 session 的输出/终态，Agent wait 只响应
协作通知和 durable facts，两域互不唤醒、互不 claim，也不提供 wait_any。

wait first-ready 把 message item 与完整 target/status/display snapshot 冻结到同一个 batch receipt，并推进
caller/run/target cursor；重试同一 batch 返回首次冻结快照。状态版本由 Agent lifecycle revision 加所有
Wake `status_revision` 的单调和构成，所以较新的 queued/satisfied 事件不能遮住较老 active Wake 的后续
终态，disabled/archived 也能被观察；lease-only owner 变化不算语义版本。display 仍按 active-priority
repository 派生，而不是拿最新 Wake 冒充当前状态。

wait 的 first-ready 事务不留下“cursor 已推进、ToolResult 尚未落库”的窗口：它冻结 item/target 快照，
把同一个 `wait_agent` ToolCall 的确定性 ToolResult 与 model-context 前缀持久化，并设置
`sampling_bound_at`，然后才返回 `AgentWaitModelProjection::PrecommittedToolResult`。第 4 轮 adapter 必须
按 receipt 重载这段 durable prefix，跳过 Runtime 通常会做的第二次 ToolResult record；它不能重读 target
inbox、重新拼结果或把返回值当普通未持久化 ToolResult。若进程在较早的 open receipt 阶段退出，下一
Turn 通过不可变 replay link 读取完整的认证 message 与 status 快照，在同一个事务中提交新 ToolResult 并
关闭 source/current receipt；因此 message-bearing 与 status-only wait 都不会因换 run 永久搁浅。

wait 最多接受 32 个不同 target；候选先按整组 Mailbox sequence 做全局 FIFO，再应用每次最多 64 条的
预算，不能因 target 排序越过更早消息。单条 payload 复用 32 KiB 认证 envelope 的确定性截断；message
projection 预留 96 KiB，包含 status 和 ToolResult 外壳后的最终 durable JSON 硬限制为 128 KiB，模型侧
再统一经过现有 10k-token ToolResult gate，超预算消息继续 pending 到下一次 wait。

双等待集成测试使用真实 `AgentCommandSessionRegistry` 的生产 Condvar 和持久 Command Session
record/read receipt（执行进程由无 Shell 的测试 Host 替代）：命令 settle 不唤醒或消费 Agent wait，Agent
result 也不结束 Command wait，两边随后都能从各自真相源取回结果。关停信号只结束当前 wait；不删除
Mailbox、target、Command Session 或稍后到达的结果。

### 第 4 轮：Harness、权限审批与跨进程协议

只提供 `spawn_agent`、`send_message`、`followup_task`、`wait_agent`、`list_agents`、
`interrupt_agent` 六个工具；完成根树权限、Approval 路由、严格 DTO、Host/Main/Preload 接线。

#### A：六工具与窄 Host 边界

Core 只声明六个严格 JSON schema 和 `AgentCollaborationExecutor` Host port，不引用 SQLite、
Dispatcher、RPC 或 UI。只有 Host 注入可信 `AgentCollaborationCaller` 时才整组注册六个工具；
未提供协作 capability 时一个也不出现。Server 在真实 root Turn 构造时对旧 Conversation 执行
幂等 `ensure_root`，child 则从 Agent 节点重建身份；模型参数里不存在 sender/root/workspace 字段。

- `spawn_agent(task_name, message, agent_type?, model?, reasoning_effort?, fork_turns?)` 只建直接子节点；
  `agent_type` 是 machine key，`model` 是 `model_config_id`，两者都是精确匹配。
- `send_message(target, message)` 只持久入队；`followup_task(target, message)` 额外保证最终执行机会，
  但永不给正在运行的目标开第二个 Turn。
- `wait_agent(targets, timeout_ms?)` 是 first-ready，0 表示立即快照，默认 30 s，最长 300 s，最多
  32 个目标；它复用第 3 轮 durable receipt，不重读目标 inbox。
- `list_agents()` 只返回授权树的简洁投影；`interrupt_agent(target)` 只中断当前 Turn，不删节点、
  Conversation 或历史。不增加第七个工具。

Host 每个 Turn 提供当时快照的脱敏 selector 目录：已启用模板的 machine key/名称/简介/
模型显示名，以及可用 model config id/显示名。目录按稳定 key 排序，每类最多 32 项、
总 JSON 最多 16 KiB，确定性截断并标记 `truncated`；类型上无 API key、URL、Provider 配置或模板
instructions。目录只帮模型选择，过期/被截断 selector 仍由后端严格返回 unavailable，不模糊
匹配。目录插入系统提示词前把 `<`、`>`、`&`、反引号和 Unicode 行分隔符编码为可逆 JSON
Unicode escape，防止用户可编辑的名称/简介闭合数据标签或打开 Markdown 指令边界；16 KiB 总上限按
编码后的实际字节重新执行，解码后的 selector 值不变。实际可调用 selector 集合与目录来自同一份
Host-authored snapshot，并随 Turn checkpoint 持久化；Approval continuation 或进程恢复继续使用原 Turn
的授权快照，不能在恢复时重算目录后获得模型从未看到的新 selector。

可能跨过持久副作用边界的 spawn/send/follow-up/wait/interrupt 都使用 authoritative
cancellation settlement，不把已提交的 Host future 遗留在 Run 外。`wait_agent` ready 返回时 ToolResult 已和
cursor/model batch 在同一持久事务提交；Runtime 只更新内存 trace，不再次发布相同持久前缀。稳定
错误分类为 `invalid_arguments / permission_denied / conflict / unavailable / unsupported /
resource_limit`，由 typed 领域错误映射，不从文案猜测。同一个 Provider tool-call batch 只允许一次
`wait_agent` admission；第二次调用在进入 Host 前稳定失败，并由普通 Runtime 路径持久化自己的失败
ToolResult，不能复用第一条 `(run, model batch)` delivery receipt。

Core Server 在 outbound 事件通道建立后、开始接收请求前启动唯一进程级 Dispatcher。启动动作会立即
扫描 SQLite 中已有的 queued/recoverable Wake，因此上个进程未收到或来不及处理的内存通知不影响恢复；
关停时由同一个 `AgentService` owner 取回 Dispatcher 并走有界收敛。确定性 fake-provider 集成测试通过
真实 Tool schema → Runtime → Host port → application service 链同时运行两个使用不同模板/模型的 child，
并锁定旧 root 懒物化、恰好六工具、Conversation 隔离和 wait precommitted ToolResult 只写一次。

#### B：统一授权、资源边界与根 Approval 投影

`CollaborationAuthorizer` 是 renderer RPC、Harness adapter 和应用服务共用的授权边界。它只从
`agent_nodes` 解析 caller/root/project/parent，不接受前端或模型提交的 `isChild`、sender、root、task
path 作为权限事实。旧的未绑定 Conversation 保持原行为；一旦绑定 Agent，普通用户写入只允许 root。
child 只可通过携带 exact root Conversation 和 child Conversation 的 observer RPC 读取，普通历史、meta
和全文搜索均过滤 child。start/steer/stop/fork/provider transition、Approval decision、消息 edit/retry/
delete、meta/UI state/composer draft 等旧入口在执行写操作前都经过相同 root guard；持久 Agent tree 的
Conversation/project 不能通过旧删除入口被级联破坏。

当前 canonical 外键将 Agent-bound root Conversation/Project 的旧删除请求显式拒绝，而不是猜测级联范围；
安全的“归档/删除整棵树”需要独立生命周期事务，不能借旧单 Conversation 删除 RPC 顺手实现。普通未绑定
Conversation/Project 的删除行为保持不变。

普通 send 允许同树定向；follow-up、wait、interrupt 只允许 caller 的严格后代；不存在、跨项目和跨树
target 对外统一表现为 permission denied，不能充当存在性探针。默认产品限制是深度 8、每树 64 节点、
Harness 消息 64 KiB 和 Dispatcher 全局并发 4。节点数/深度在 child spawn 的同一个
`BEGIN IMMEDIATE` 内校验，不能由两个并发 spawn 同时越过；既有 request 的幂等 retry 在配额检查前返回
原记录。Mailbox 仍保留第 3 轮的持久硬配额。模板当前没有权限字段，因此视为中性上界，不能把
`AgentPermissions::default()` 误当模板策略强行降权。root/child Turn admission 在同一
`BEGIN IMMEDIATE` 中写 `agent_effective_permission_snapshots`；旧根首次协作时由可信
`ToolExecutionContext` 在任何 Harness 动作前补齐 root snapshot，模型 payload 没有权限字段。child 新
Wake 以直接父 snapshot 为基线，再与完整祖先链逐字段取交集；缺任一 snapshot 都拒绝启动。未来模板、
项目或动态策略只有接入真实策略来源时才作为额外上界参与 meet。

child Approval 不复制审批表或状态机。`agent_pending_actions`、checkpoint、原 child Run 和 action audit
继续是真相；renderer 看到的是按 root tree JOIN 得到的逻辑投影。稳定 `approval_id` 是后端 framed
pending-action id，根决策请求只携带 `root_conversation_id + approval_id + decision`，server 反查 source
Agent、run/action 并调用原 Approval continuation。重复、过期或已终结点击只返回 durable 已结算状态，
不启动第二个 continuation；重启后投影由 pending journal 重建。pending INSERT/状态 CAS 与 root-tree
collaboration event 由 canonical trigger 同事务提交，内存通知仅用于刷新。

#### C：稳定协议、持久事件与 renderer 数据层

`agent.collaboration.*` 是严格、版本化的小 DTO 面：树/节点详情、精确 child Conversation observer、
模板 CRUD、根 Approval 投影和 event replay。DTO 只包含模型/界面安全的 display 信息；不返回连接 URL、
credential、Provider profile、内部 lease/claim token、checkpoint 或完整系统提示词。Main 对请求和响应都做
runtime parser 与 root/agent/project 身份回验，Preload 仅开放固定 allowlist。普通 `storage.load*`、历史和
搜索继续隐藏 child；observer 必须同时提交 exact root Conversation 与 child Conversation。

`agent_collaboration_events` 是按 root tree 分区的 append-only invalidation outbox，不是第二份聊天或状态表。
每次 Agent、Mailbox、Wake、Turn 和 child Approval 状态更新都由 canonical trigger 在同一 SQLite 事务里
推进 root-local sequence 并写 event；数据库触发器同时验证 root、Agent、Conversation 和 sequence cut 的
精确关系。进程内 notifier 只降低延迟；崩溃、丢通知、重复通知或窗口重载均通过
`listEvents(afterSequence)` 从该日志补洞。全局 sequence 只供 server notifier 扫描，renderer 永远按
root-local sequence 合并，两个并行子 Agent/不同 root 不会串线。

语义 activity 由同一 canonical trigger 在领域事务内生成；Renderer 不从摘要或模型文本猜测：

| 持久领域 mutation                                                                                                       | `activity.semantic`    | 约束                                                                 |
| ----------------------------------------------------------------------------------------------------------------------- | ---------------------- | -------------------------------------------------------------------- |
| initial task / follow-up 对应的 Wake INSERT                                                                             | `started`              | 仅真实 `task` / `followup` source；result Wake 不产生                |
| direct child → parent 的非终态 `message` Mailbox INSERT                                                                 | `updated`              | activity subject 是 sender child；outer event subject 仍是 recipient |
| child pending Approval INSERT                                                                                           | `waiting_approval`     | 保留原 child/run/assistant identity                                  |
| Wake 以 final result 原子进入 `completed` / `failed`                                                                    | `completed` / `failed` | result Mailbox 本身不再额外产生 `updated`                            |
| Wake 持久进入 `interrupted`，或 queued/claimed-before-admission 被管理中断写为 `cancelled`                              | `interrupted`          | 两者都是用户语义上的中断，不依赖内存 cancel 通知                     |
| 除 direct child → parent 外的普通 send（含 parent → child / sibling）、result、Mailbox claim/ack、Wake lease、list/wait | `null`                 | 仍写必要 invalidation event，但根聊天无 activity                     |

Core 进程启动时先冻结当前 durable global cursor，再发一次严格的全局
`agent.collaboration.resync(core_started)`，随后只从该 cursor 之后订阅 outbox。这样现存 Main/renderer
不会把重启前后窗口误当成“已经同步”；cursor 冻结后提交的 mutation 仍由 event replay 覆盖。模型设置
是跨 root 的显示元数据目录，成功提交后发窄的
`agent.collaboration.resync(model_settings_changed)`；失败事务不发通知。resync 只是失效信号，renderer
仍从数据库重新 hydrate，通知本身不携带或替代任何领域状态。

树 snapshot 在读取多查询状态前先取得保守 replay cursor，因此并发 mutation 最多被重复 replay，绝不会
让 cursor 越过 snapshot 尚未观察到的状态。未懒物化 root 的旧 Conversation 返回
`materialized=false/tree=null`，是健康的“无协作”状态；真正存储/RPC 失败保持 error 并在下一 durable
notification 或显式 hydrate 时重试。

renderer 的 `CollaborationStore` 只保存 root-scoped Agent tree/display 索引和 bounded semantic activity
projection，不保存 Conversation 消息或 reducer。notification 是失效提示：store 忽略重复/其他 root，
按持久日志验证连续 sequence，遇缺口或乱序后重新水合数据库 snapshot；窗口 reload 新建 store 后直接
水合数据库。observer Conversation 使用
专用严格 DTO，保留安全的完整 chat display 投影（attachments、Agent run/UI state）并为每个 user-role
输入附带可信 human/agent/historical-snapshot origin；UI 不得把 agent-origin 显示成“你”。

跨进程协作基础 DTO 仍为 `schemaVersion=1`，带必填 nullable activity 的 event envelope 独立升至
`schemaVersion=2`。第 4 轮冻结的 canonical storage 是 v7；第 5 轮为根 Agent Conversation 的
provenance-aware fork authority 升至 v8；持久权限快照和语义 activity projection 将当前基线升至
v9。v8 及更早开发库继续采用既有 reset-required、原库不改写策略。

### 第 5 轮：前端复用与完整用户体验

把现有聊天 Surface 拆成 interactive/observer 两种模式；子 Agent observer 放在右侧栏指挥中心，
复用消息、Markdown、Tool、Artifact、错误、stream 和 Usage 展示。用户仍只操作根 Agent。

协作能力懒物化 root 后，根 Conversation 继续新任务使用唯一既有 `conversation_forks` receipt 的
`collaboration_root` authority：同一 `BEGIN IMMEDIATE` 创建目标 Conversation、独立 root Agent、fork
receipt 和带来源的历史 snapshot。Agent-origin 输入保留 original Agent/Mailbox provenance，在面向用户的
根历史与搜索中隐藏、在 Context/审计中保留；子 Conversation 和活跃 Turn 仍 fail closed。重复 request ID
只按不可变 receipt/根身份返回原目标，不受之后 lifecycle 展示状态漂移影响。

后台 child 没有 renderer 写入 `agent_run_json` 时，observer 展示由 server 从 durable trace、Command
Session、Skill 与 MCP action journal 生成 renderer-safe projection。MCP 参数、结果正文和 diagnostics 不
进入该投影；旧版本已经 scrub 且从未落安全 invocation receipt 的 MCP 历史降级为 generic Tool，不伪造
执行身份。

### 第 6 轮：可靠性、迁移与发布门禁

完成崩溃/重启、重复请求、乱序事件、Approval、删除、模型不可用、并发压力和 Command
Session/wait_agent 交叉测试；验证旧 schema 策略、备份恢复、E2E 和完整质量门禁。

每轮只实现其明确边界。后续轮次可以依赖前一轮公开的领域与 application service 接口，但不能
预先把后续业务塞进第 1 轮，也不能以“未来通用性”为理由扩大成工作流平台。
