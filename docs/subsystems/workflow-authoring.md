---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-10-02
---

# 工作流邮件网络

工作流是共享背景、独立对话和用户成员组成的邮件网络。成员可以按职责自由通信；布局只表示成员位置，不决定执行顺序。没有永久连线、入口流股、输入/输出逻辑门、批次门槛或排队/插入配置。本文描述当前工作树，不表示这些改动已随公开版本发布。

## 定义与编辑

开发期 `schemaVersion: 1` 定义仅含 `id`、`name`、`description`、`background`、`nodes`、`viewport` 和版本字段。节点最多 128 个，定义大小、目录容量及乐观并发限制仍由 Rust Core 校验。坐标必须有限，ID 必须唯一。

- `agent`：ID、名称、坐标、`modelConfigId`、`permissionMode`、`receives`、`task`、`delivers`。
- `user`：ID、名称、坐标、`task`。用户头像和显示名称来自账户上下文。

不接受 `flows`、`boundaryPositions`、`nextFlowSequence`、门节点或消息接收策略。没有旧定义转换或缺省补门。无效记录仍按现有目录机制隔离，避免损坏条目阻断整个目录；不会自动重写旧数据。

设置中的「基本信息／流程设计」、名称/权限/模型顶部配置行和节点基本配置面板保持原有设计。接收说明始终可编辑，无需先连线；没有新增接收配置页。添加菜单只提供智能体与用户，空画布没有合成用户入口。可拖动、缩放、适应画布、删除、撤销/重做；Command/Ctrl+C/V 复制节点全部配置，产生独立 ID。文字输入框保留原生复制粘贴。平移缩放不占内容撤销步骤。

模型及权限是节点的默认值，绑定对话后复用既有对话配置。保存模板不创建对话；确认激活实例时才为未绑定智能体创建对话。可选所属项目只决定新建对话的位置，不限制跨项目绑定。

## 状态所有权与工作区

模板、实例、绑定、邮件和工具幂等性由 Rust Core 持有；Renderer 只编辑草稿、展示状态及处理视口。工作流实例没有单独的模型 Run，每个智能体仍是独立根对话。子 Agent 协作邮箱与本系统不同，不能互换授权或队列。

模板的 `enabled` 是当前校验是否通过；实例 `enabled` 表示用户是否开启；`running` 来自真实对话活动。三者不互相替代。保存、启停、删除、草稿和模板使用检查保留 revision/CAS。唯一绑定、开启颜色占用、归档保护、失效关闭和未保存编辑保护保留。

工作流首页、绑定配置和看板统一位于右侧栏。左侧工作流入口打开首页并最大化；已绑定对话的入口打开所属看板。双击节点在中央打开真实对话并恢复侧栏宽度。对话顶部菜单展示其他已绑定协作成员，不再区分上下游。切换聊天或标签保留画布状态。

## 邮件生命周期

每次发送为每位收件人创建独立、不可变的消息信封和一个持久输入收据，不再跨消息组批。信封保存稳定消息 ID、工作流、发送/接收节点和当时绑定的对话名称、正文、时间及可选 `replyToMessageId`。模型不能自行填写发送者身份。

| 邮件状态     | 含义                                     |
| ------------ | ---------------------------------------- |
| `pending`    | 未领取，保留在收件箱中                   |
| `processing` | 已被当前轮次领取，等待正式投递或正在处理 |
| `processed`  | 模型显式完成，或持有它的轮次正常结束     |
| `stopped`    | 持有它的轮次被停止                       |
| `failed`     | 投递或处理轮次失败                       |
| `recalled`   | 发件人在尚未领取时撤回                   |

`processed` 表示本系统的处理结束，不证明业务结果正确。内部 input 的 pending/claimed/applied 等状态仅记录传输事实，不能直接充当前端邮件状态。历史失败邮件不阻塞后续邮件。已处理、已撤回的状态不会被后续轮次停止、失败或恢复覆盖。

### 唤醒与主动领取

1. 来信写入邮箱并通知调度器。运行中的节点不会自动插入其他来信。
2. 节点处于普通空闲状态时，调度器按到达顺序领取最早的一封，走现有根 Turn 准入并唤醒它。
3. 轮次中模型通过邮箱查询了解剩余来信，再用 `workflow_accept` 主动领取任意待处理邮件。领取不新建 Run，正文在同轮下一次安全采样边界正式交付。
4. 轮次正常结束时，仍归属该轮次的 processing 邮件自动变为 processed；模型也可随时调用 `workflow_complete` 完成已正式交付的指定邮件。
5. 回到空闲后仍有 pending 邮件，就再次按顺序取一封唤醒。没有未处理邮件时保持空闲。

用户点击停止会保留停止屏障，后续邮件留在邮箱，不立刻唤醒。用户手动发送新消息恢复该对话后继续正常机制。审批、等待用户交互、运行中或并发名额不足均不视为可自动唤醒的空闲。工具查询和 World State 更新不恢复停止状态。

用户节点也是邮件收件人，但不会调用模型。收到邮件时显示等待用户操作，点击后通过「我已完成」确认指定邮件；不自动发出回复。

## 六个动态工具

所有工具仅在已准入、仍有效且开启工作流的独立根 Run 中挂载。实例停用、模板/绑定失效后下一次采样移除；每次工具执行仍复验冻结的 Run 身份和当前成员资格。历史消息、checkpoint 或子 Agent 不能自行取得工作流授权。

| 工具                   | 参数与作用                                                                             |
| ---------------------- | -------------------------------------------------------------------------------------- |
| `workflow_send`        | `{ messages: [{ targetNodeId, message, replyToMessageId? }] }`，直接向同工作流成员发信 |
| `workflow_get_state`   | `{ reason, view?, nodeId? }`，按需查询成员/运行信息；view 为 members、runtime、all     |
| `workflow_get_mailbox` | 自己的 inbox/outbox；支持消息 ID、输入 ID、状态和游标过滤                              |
| `workflow_accept`      | `{ messageIds }`，将自己的 pending 邮件领取到当前轮次                                  |
| `workflow_complete`    | `{ messageIds }`，将当前轮次已正式接收的邮件标记已处理                                 |
| `workflow_recall`      | `{ messageIds }`，撤回自己发送且仍 pending 的邮件                                      |

不提供退回工具。模型需要补充材料时可发一封说明问题的新邮件，并用 replyToMessageId 关联原信。成员资格允许通信，不产生新的文件、命令或其他工具权限。

邮箱查询返回 pending 邮件的真实正文，读取本身不领取，也不改变未来自动唤醒。工具结果明确提醒模型：仅查询过的邮件仍会投递；决定现在处理时必须主动领取。模型没有新成果、信息或必要交接时可以不发信，禁止为了维持网络活跃而重复发送占位邮件。

发送/领取/完成/撤回以 Run + tool call ID 做持久幂等。发送时校验收件人属于当前实例，replyToMessageId 必须在自己的相关邮件范围内；相同调用 ID 携带不同请求拒绝执行。领取与撤回竞争使用同一事务和权威状态，已经领取的邮件不能再撤回。

主动领取的工具记录保留正文供 UI 展示，但模型结果及历史工具结果投影去除重复正文；正式内容通过单独 `WorkflowDelivery` 进入上下文，避免领取回执和正式投递双份注入。邮件是协作资料，不能冒充用户指令或授权。聊天气泡是 Host 投递记录的展示投影，不能再次作为普通 HumanText 重放；分叉保留来源展示，不继承原工作流成员资格。

## World State 与 harness

每次采样准备时更新三个 Host 所有的分区：

- `workflow.execution`：冻结成员身份、公共背景、当前节点职责和成员列表。
- `workflow.awareness`：精简的实际活动/邮箱概况，详细状态按需查询。包含 pending/processing 数、稳定序号与累计收到邮件数。
- `workflow.mailbox`：自上次模型实际观察以来的新邮件简短提示，使用累计到达数与序号，而非 pending 数差计算。

提示复用已有 World State observed request 提交：预览、失败请求和只准备未发送的请求不消费新信通知；请求成功观察才确认该次截点。到达后又被领取/撤回的邮件不会因 pending 总数相同而漏报。刷新摘要不单独启动模型，也不把全部邮箱正文推入上下文。

`workflow_get_state` 不重复公共背景，运行结果可相对调用时已观察摘要省略相同概况，指定成员时提供所需细节。邮箱可按消息 ID 取正文；分页和正文预算必须明确标记，不能静默截断后推进游标而丢信。收发邮箱严格限定当前成员的收件人/发件人身份。

终态邮件结算挂在 `conversation_trace_repository::commit_trace_with_loaded_prefix`，与权威 Turn trace 同事务提交，包括正常结束、停止、人工交互终止和启动恢复。没有正式投递证明的已领取邮件不能被正常结束误标成已处理。持久输入、run/delivery 绑定和 trace 共同提供恢复去重。

调度器独立于前端：合并通知、按 sequence 分页推进可投递会话，配置/容量/权限暂不可执行时使用既有退避机制。模型配置变化、名额释放和新邮件会触发重新检查。每次仍走权威准入；没有自动 injection 通道，当前 Run 的额外 WorkflowDelivery 只能来自已领取邮件。

SQLite v64 新增独立 `workflow_mail_*` 表，保存信封、输入、发送/操作回执、事件、Run 身份、来源和停止屏障。安装只增加空表及索引，不转换旧工作流数据，不重置项目或聊天。定义 schema 与存储 schema 独立。

## 前端与实时看板

模板、绑定画布和看板均没有永久边或逻辑门。节点左侧使用邮箱图标和未处理数量，头像在其右侧；节点悬停仍只显示原有角色、模型、权限和任务信息。

看板只根据真实 `sent`/`recalled` 事件短暂画连接，流水灯按事件中源/目标方向移动后消失。撤回事件由后端反向给出端点。领取/完成是本地操作，不伪造跨节点传输。事件按实例序号去重，首次打开、重新打开和隐藏期间不重播历史；减少动态效果时不播放流水动画。动态线不写入定义，不影响撤销或布局。

单击节点打开可收起的详情，显示待处理、处理中和历史邮件；双击仍打开对话。正文按节点分页读取，运行快照只传无正文的元数据。显示六种邮件状态，不用 runStatus 再猜测或覆盖邮件完成结果。

工具展示延续现有折叠状态栏：

- 状态查询不可展开，显示「已查看工作流 · 调用理由」。
- 收件箱/发件箱有明确「工作流」前缀，共用邮箱底座与向内/向外箭头；空结果不可展开。
- 领取、完成、撤回各有操作图标和状态文案，展开可见对应具体邮件、复制与对话跳转；部分失败不显示全成功。
- 发信展开显示发送正文与收件成员，名称来自 Host 信封，不从 UUID 或正文猜测。
- 不向用户显示 JSON、执行版本或技术详情，历史结果按当时回执呈现。

## 代码真源与验证

- [定义协议](../../packages/protocol/src/workflows.ts)、[运行协议](../../packages/protocol/src/workflowRuntime.ts)、[跨语言 fixture](../../packages/protocol/fixtures/workflow-definition-v1.json)。
- [领域校验](../../crates/core/src/workflow.rs)、[邮件契约](../../crates/core/src/workflow_execution.rs)、[查询契约](../../crates/core/src/workflow_awareness.rs)。
- [持久邮箱与操作](../../crates/core/src/storage/workflow_execution_repository.rs)、[投递与结算](../../crates/core/src/storage/workflow_execution_repository/delivery.rs)、[查询](../../crates/core/src/storage/workflow_execution_repository/awareness.rs)。
- [动态工具扩展](../../crates/core/src/runtime/extensions/workflow.rs)、[World State](../../crates/core/src/workflow_runtime.rs)、[Host 协调](../../crates/core-server/src/application/agent/workflow_execution.rs)。
- [模板配置](../../src/renderer/src/features/workflows/WorkflowSettingsSection.tsx)、[实例页面](../../src/renderer/src/features/workflows/project/WorkflowsPage.tsx)、[看板](../../src/renderer/src/features/workflows/project/WorkflowMonitorPage.tsx)。

验证重点为自由通信、邮箱正文可见但查询不领取、领取/撤回竞争、幂等、同轮主动完成、终态原子结算、停止屏障、休眠 FIFO 唤醒、动态工具授权、observed 差量提示和 UI 事件去重，同时保留节点编辑、项目绑定、复制粘贴、撤销、缩放和对话导航的回归覆盖。
