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

带来源消息的 Wake 只有在对应 Mailbox message 已完成唯一 Conversation projection 并确认后
才能入队；因此 Dispatcher 后续不会先启动 Turn、再发现模型上下文里还没有任务正文。
对会触发执行的 task/followup，repository 提供“写投影并确认 + 创建唯一 Wake”的单事务 API；
普通 message 可以只投影确认。同一 source message 最多绑定一个 Wake，崩溃后可按幂等 request
重试，不能出现“正文已经投影但唤醒事实永久缺失”的半完成状态。

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
- 子 Agent 不继承超过根 Agent 的权限。模板不能提升权限。
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

### 第 3 轮：协作运行时、Mailbox 调度与双等待

实现受限并发 Dispatcher、每 Agent 单 Turn、持久 Wake 恢复、父子消息安全注入，以及
`wait_agent` 与 Command Session wait 相互独立、均不丢持久结果的语义。

### 第 4 轮：Harness、权限审批与跨进程协议

只提供 `spawn_agent`、`send_message`、`followup_task`、`wait_agent`、`list_agents`、
`interrupt_agent` 六个工具；完成根树权限、Approval 路由、严格 DTO、Host/Main/Preload 接线。

### 第 5 轮：前端复用与完整用户体验

把现有聊天 Surface 拆成 interactive/observer 两种模式；子 Agent observer 放在右侧栏指挥中心，
复用消息、Markdown、Tool、Artifact、错误、stream 和 Usage 展示。用户仍只操作根 Agent。

### 第 6 轮：可靠性、迁移与发布门禁

完成崩溃/重启、重复请求、乱序事件、Approval、删除、模型不可用、并发压力和 Command
Session/wait_agent 交叉测试；验证旧 schema 策略、备份恢复、E2E 和完整质量门禁。

每轮只实现其明确边界。后续轮次可以依赖前一轮公开的领域与 application service 接口，但不能
预先把后续业务塞进第 1 轮，也不能以“未来通用性”为理由扩大成工作流平台。
