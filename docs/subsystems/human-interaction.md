---
status: current
audience: developers
owner: engineering
last_verified: 2026-09-06
---

# 向用户提问：设计与实施进度

本文记录完整产品契约及分轮交付状态。前两轮完成基础设施与阻塞式提问；第 3 轮接入异步工具、多批次回答及可靠投递。第 4 轮接入独立设置、分页面板、抢占协调和问答气泡；第 5 轮完成跨层验收、问题修复、schema v39 和开发/用户文档交付；实际测试与未验证边界见文末。

## 产品契约

- 人机交互支持补充信息、表达偏好、判断/决策、反馈，以及需要用户亲自参与的操作；用户明确要求交互时也可使用。选项按具体事项组织，没有固定回应模板。
- `request_user_input` 挂起当前根智能体，整批回答作为原工具调用的唯一结果恢复同一逻辑 Run。
- `request_user_input_async` 接纳后立即返回，继续独立工作；整批回答随后作为用户消息投递。
- 两工具只属于直接面向用户的根智能体。普通单 Agent 聊天属于根聊天；所有层级子 Agent，
  包括自身拥有后代者，均不得挂载或执行。第一阶段不向无人值守 Automation 挂载。
- 设置 → 个性化新增“人机交互”及默认开启的“允许智能体向人类提问”。Host 独立保存设置及
  revision，单次输入中的 Prompt Preferences 不具备覆盖此策略的权限。
- 关闭只阻止新提问，已有问题仍可提交或忽略。创建问题与关闭设置在同一数据库写事务边界裁定先后。
- 每题一页，不设置产品题数上限。允许多个未结束的异步批次，一次工具调用对应一批问题。
  整体请求字节数、字段长度和输入合法性仍有技术限制。
- 每题在选项、非空自由文字、“跳过”之间选择；选择跳过后按钮显示“已跳过”。提交前可以修改，全部处理后统一提交。
- 顶部前后箭头翻页。底部“跳过”只标记当前题，右侧未完成整批时为“下一题”，当前题有效后定位下一个未处理题（可回绕）；全部有效后切换“提交”。翻页只更新草稿。
- 展示优先级为审批 > 阻塞提问 > 非阻塞提问。审批期间不允许切回提问；结束后读取权威状态，
  恢复尚未提交批次的页码与草稿。阻塞提问抢占异步；新的异步批次优先展示，旧批次保留。
- 非阻塞面板右上角可以最小化。Timeline 每个批次只有一个重开入口，不按题创建入口。
  最小化不结算问题，点击入口恢复草稿，不能抢占审批或阻塞提问。
- 提交成功后答题卡和该批入口消失，聊天中生成一条按题序组合“问题＋答案”的用户气泡。
  问题为灰色，答案为正常颜色，“不回答”显示“已跳过”。没有完成卡片、补答或编辑入口。
- 同步、异步共享用户气泡展示，但模型投影不同：同步只通过原 ToolResult 进入模型，
  禁止把展示气泡再次拼成普通 User 消息；异步通过 guidance 或新 HumanRoot Turn 进入。
- 非阻塞独有“忽略全部”，只忽略当前批次，不发送草稿、User、guidance 或 Wake，
  不激活模型，不影响其他批次。只能在下一次自然发生的采样中附带已忽略状态。
- 每题都选“不回答”后点提交仍是正式回应，不能转换为忽略。
- 原运行自然完成后，异步问题继续有效；整批提交在同一聊天激活后续运行。
- 中文输入法确认、文本框 Enter、Escape 不能意外提交、跳过、忽略或停止运行。
- 前端复用现有审批框的视觉变量，但不继承其立即批准和 Escape 取消等业务行为。

## 所有权与挂载

公共领域类型在 [`human_interaction.rs`](../../crates/core/src/human_interaction.rs)。模型只传
`questions: [{title, options?}]`，批次、问题、选项 ID 及 conversation/run/toolCall 归属由 Host
生成或核验。数据库创建入口不作为 Renderer RPC 暴露。

`HumanInteractionExtension` 贡献动态能力和 RequestOnly 使用说明；同一次模型请求的工具定义、
提示词和能力状态使用同一策略快照，不改动稳定工具前缀。Host 提供私有策略服务，Harness 再检查
子 Agent/Automation 身份。未完成执行链路时 execution readiness 关闭，即使设置开启也不得向
真实模型提供不可执行的 Schema 或专项提示词。设置开关与实现 readiness 是不同事实。

工具描述区分“必须等待用户参与才可继续”和“仍有独立工作可做”。能够自行处理的事情自行处理，
需要用户独有信息、判断或实际协助时清楚请求。回应必须结合原交互含义解释，偏好不是授权，意向不是行动结果，
用户陈述不能写成智能体执行或验证所得的事实；拒绝、跳过、忽略和沉默均不表示同意或请求事项已发生。交互不能替代正式权限审批。

## 交互提示词（2026-09-06）

两项工具从仅用于信息/偏好问答扩展为请求用户参与任务，包括判断、决策、反馈和实际协助。
不新增工具、数据库状态或参数字段，继续使用 `questions[].title/options` 和 `option/text/skipped`；
标题可表达问题或协助请求，选项随事项变化，不预设固定的完成、拒绝等回应模板。

- 英文模型工具描述及字段说明位于 [`tools/human_interaction.rs`](../../crates/core/src/tools/human_interaction.rs)。
- 中文专项提示位于 [`runtime/extensions/human_interaction.rs`](../../crates/core/src/runtime/extensions/human_interaction.rs)，共享交互组织和回应解释规则，仅路由段依据同步/异步 readiness 组合命名可用工具；依旧按当次能力快照注入 RequestOnly System 上下文，关闭时撤下，压缩采样不注入。
- 稳定系统提示 [`prompts.rs`](../../crates/core/src/prompts.rs) 的交互原则允许实际协助及用户明确要求交互，不硬编码动态工具名称。权限规则继续要求正式审批/有效设置，不允许通过用户代做绕过受限操作。
- 下一次自然采样的忽略说明在 [`runtime/preparation.rs`](../../crates/core/src/runtime/preparation.rs) 中统一为“异步交互状态”，明确没有提交回应，也不能把忽略解释为请求事项已发生。它不会额外调用模型。

回应解释以原事项和用户实际表达为依据；只作回应能够支持的判断，不将偏好扩大为授权、意向视为行动结果、用户陈述视为自身执行证据。后续依赖可核验外部状态时，在现有授权能力内进行必要核验；不能核验时说明事实来源及不确定性。

本次验证（2026-09-06）：

- `cargo test --locked -p mycopilot-core human_interaction --lib`：77 项通过。包括可控 Provider 的真实请求内容、开关与 schema/专项提示同快照、同步/异步单独可用的提示词裁剪、根/后代/Automation 边界及原有回应投递回归。
- `cargo test --locked -p mycopilot-core prompts::tests --lib`：18 项通过，通用交互规则不再限于缺失信息，且不硬编码动态工具名。
- 首次编译被联网扩展测试访问私有字段阻挡；仅将该测试改用现有 ContextFrame 投影和 AgentError 访问接口，联网执行行为未改。`cargo test --locked -p mycopilot-core runtime::extensions::web_search --lib`：4 项通过。
- `cargo clippy --locked -p mycopilot-core --lib --tests -- -D warnings`、本次提示词 Rust 文件的 rustfmt、开发/用户文档及 diff whitespace 检查通过。未调用商业模型、未修改数据库版本或重置数据；测试验证提示词传递与契约，未将其表述为商业模型行为评测。

## 数据与状态

- Settings：全局 enabled/revision/updatedAt，默认 true/0/0，独立于 Prompt Preferences。
- Request：Host 创建 sequence、模式、可信 owner、题目快照、policyRevision、版本与状态。
- Response：一次不可变的整批回应及提交身份。正式回应按原题序归一化；忽略不包含草稿。
- Delivery：与 Response 一对一的回答投递事实，区分 pending/bound/applied/cancelled/failed。
- Suspension：同步恢复所需的私有检查点和绑定，不进入公开快照或普通 IPC。
- Async binding：异步回应的私有路由、领取身份、停止范围与执行阶段，不允许 Renderer 指定。

基础五张表为 `human_interaction_settings`、`human_interaction_requests`、
`human_interaction_responses`、`human_interaction_deliveries`、`human_interaction_suspensions`。
第 3 轮增加 `human_interaction_async_bindings`，与既有 guidance journal、Turn admission 和模型请求记录协作。
Request 的公开 sequence 固定批次创建顺序，列表及分页游标按它倒序；同毫秒创建、重连或删除
部分历史后也能确定最新批次。Response 的独立数据库 sequence 固定回答接纳顺序，不能用批次
展示次序、前端页码或毫秒时间猜测投递次序。两个序号均由 Host 生成，提交输入不能指定。
同步挂起表保存 Host allowlist 恢复信封、原调用绑定、领取身份与执行阶段。第 2 轮使用真实检查点驱动恢复，详情见下文。

批次只能从 open 进入 submitted/ignored/cancelled。只有异步支持 ignored。
submitted 不等于 applied；后续投递失败不能使问题重新变回可编辑。提交与忽略通过同一 revision
CAS 仲裁，已成功的同一 submissionId 重试返回已有结果；相同身份不同内容必须冲突。
正式提交创建 pending delivery，忽略不创建 delivery，也不能以通知触发推理。

问题创建和开关更新由同一数据库事务序列化。回答既有问题不再检查当前 enabled 或要求
policyRevision 未变化。批次题目 immutable，回答的 questionId 必须恰好覆盖所有题目，
option 必须属于该题；未知字段、缺题、多题、重复题、混合答案类型和空文字均拒绝。

## Host 接口

独立 `host.humanInteraction`，调用返回 `HostInvocationResult`：

| 方法           | 输入                                                               | 输出              |
| -------------- | ------------------------------------------------------------------ | ----------------- |
| getSettings    | 空对象                                                             | Settings          |
| updateSettings | enabled、expectedRevision                                          | Settings          |
| listRequests   | conversationId、cursor、limit                                      | items、nextCursor |
| submit         | conversationId、requestId、expectedRevision、submissionId、answers | RequestSnapshot   |
| ignore         | conversationId、requestId、expectedRevision、submissionId          | RequestSnapshot   |

Core Server RPC 对应 `humanInteraction.getSettings/updateSettings/listRequests/submit/ignore`。
`humanInteraction.settingsChanged` 与 `humanInteraction.requestChanged` 通知携带完整当前快照，
仅在写事务成功后发布；重连仍以查询为准。问题通知不依赖原 Run 仍然活跃。
输入和输出跨 Rust、Main、Preload、Renderer 严格校验，数字保持 JavaScript safe integer。

## 后续轮次必须维持的边界

同步恢复不能伪造 ApprovalDecision，须有独立可信 UserInput 恢复来源。保存当前 segment 用量、
暂停记录后再发布等待；等待仍占用逻辑聊天。启动对账、取消、文件事务和运行资源保留须认识
新的合法暂停原因。已完成工具不重跑，后序按冻结批次继续。

异步投递统一由 Host 仲裁：活跃可引导时在完整工具协议边界应用，真正空闲时启动后续 HumanRoot。
无 worker 不代表空闲；审批、同步等待和压缩期间先保存回答，不能绕过占用。终结竞态只能选定
旧 Run 或新 Run 之一。多个提交按 Host 接纳顺序处理，不能随前端面板顺序重排。

停止围栏优先于迟到恢复；已接收而未应用的回答在重启后不能静默丢失。结果未知的模型请求
不能靠盲目重发承诺严格一次。展示气泡、通知及 fork 都不能再次计费或重复模型投影。
Fork 只复制边界内历史，不复制活跃提问权限和投递任务；异步答案保留有界问题引用以适应压缩。

## 开发期数据策略

旧聊天、旧运行和旧问答可丢弃。只维护新 canonical schema、版本及开发 reset 流程，
不新增旧数据迁移 SQL，不提供旧 checkpoint/resume envelope 兼容。新版本自身的持久化、
重启恢复与防重仍必须实现。测试使用临时数据库，不清空真实开发数据或凭据。

Canonical SQLite 版本为 v39。正常打开旧库只返回 reset-required，不改写旧库。
受管 reset 可从受支持的 exact 旧指纹读取配置白名单后新建 v39，丢弃聊天/运行历史，保留模型配置和凭据
引用；受支持的 exact v36/v37/v38 与 current v39 reset 还保留人机交互设置及 revision。无法安全识别且含配置的旧库拒绝
重置，不能默默用默认值替换配置。既有 exact v33 私有备份配置恢复仍受 fingerprint 限制。

## 代码接续入口

| 层            | 入口                                                                                                                                                                                                         | 本轮职责                                                    |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------- |
| 公共协议      | [`human_interaction.rs`](../../crates/core/src/human_interaction.rs)、[`humanInteraction.ts`](../../packages/protocol/src/humanInteraction.ts)                                                               | 输入、答案、公开快照、校验；共享 fixture 防止跨语言字段漂移 |
| 持久化        | [`human_interaction_repository.rs`](../../crates/core/src/storage/human_interaction_repository.rs)、[`service/human_interaction.rs`](../../crates/core/src/storage/service/human_interaction.rs)             | 原子创建、结算、独立设置、Host 私有挂起材料                 |
| Harness       | [`extensions/human_interaction.rs`](../../crates/core/src/runtime/extensions/human_interaction.rs)、[`tools/human_interaction.rs`](../../crates/core/src/tools/human_interaction.rs)                         | 每请求策略快照、根身份过滤、未就绪能力关闭                  |
| Core Server   | [`application/human_interaction.rs`](../../crates/core-server/src/application/human_interaction.rs)、[`transport/human_interaction_rpc.rs`](../../crates/core-server/src/transport/human_interaction_rpc.rs) | 会话访问校验、独立 API、同步回答领取与恢复                  |
| Electron Host | [`coreServerHumanInteractionApi.ts`](../../src/main/core/coreServerHumanInteractionApi.ts)、[`HumanInteractionIpcBridge.ts`](../../src/preload/HumanInteractionIpcBridge.ts)                                 | RPC/IPC 校验、独立通知和 Core Server 重连刷新               |

## 五轮进度

| 轮次 | 内容                                                  | 状态                 |
| ---- | ----------------------------------------------------- | -------------------- |
| 1    | 公共协议、独立设置、存储事务、Host 契约、模块挂载基础 | 已完成（2026-09-05） |
| 2    | 阻塞工具、暂停恢复、用量、取消与重启                  | 已完成（2026-09-05） |
| 3    | 异步工具、多批次投递、空闲续接、忽略                  | 已完成（2026-09-05） |
| 4    | 个性化开关、分页面板、抢占、最小化与问答气泡          | 已完成（2026-09-05） |
| 5    | 跨层验收、修整、开发及用户文档                        | 已完成（2026-09-05） |

当前提交接口先保存回答，再由 Host 选择原同步调用恢复、异步 steering 或正常后续 Run；返回 submitted 不代表已 applied。
两个工具及设置页、答题面板均已接入真实 Host；前端不能直接 startTurn 绕过投递协调。

## 第 1 轮交付与验证

已完成公共领域/双语言协议、五表存储、独立设置 CAS、Host API 与通知、根智能体扩展壳。
Settings 默认为开启，但生产 `executionReady` 固定关闭，两个工具没有执行 handler，专项提示词
不会出现在真实模型请求中。设置读失败或格式错误只将此可选能力关闭，不中断普通聊天。

已执行并通过以下检查（测试筛选范围存在重叠，不应直接相加）：

- `cargo test --locked -p mycopilot-core human_interaction --lib`：35 项，包含输入边界、共享
  fixture、根/子/Automation 隔离、请求快照一致性、16 项存储并发/幂等/回滚/重读测试。
- `cargo test --locked -p mycopilot-core-server human_interaction --bin core-server`：2 项，
  覆盖独立设置 CAS、权限字段拒绝、提交后通知、pending 事实与忽略无模型消息副作用。
- `cargo test -p mycopilot-core storage::migrations::tests --lib`：30 项；
  `cargo test -p mycopilot-core-server --bin storage-reset-dev`：57 项；fork policy 专项 2 项。
  包括新库、旧库拒绝且字节不变、配置与凭据保留、不可变/终态约束以及分支不复制提问权限。
- TypeScript 单元验证：协议、Core Server adapter、IPC、Preload 和相关注册回归共 7 文件、39 项。
  命令为 `pnpm exec vitest run --project unit`，目标文件：
  `packages/protocol/src/humanInteraction.test.ts`、`src/main/core/coreServer.humanInteraction.test.ts`、
  `src/main/core/ipc.humanInteraction.test.ts`、`src/preload/HumanInteractionIpcBridge.test.ts`、
  `src/main/core/ipc.skills.test.ts`、`src/main/core/ipc.imageGeneration.test.ts`、
  `src/main/core/startupReadiness.test.ts`。
- `pnpm typecheck:node`、`pnpm typecheck:web`；
  `cargo check --locked -p mycopilot-core-server --all-targets`；
  `cargo clippy --locked --workspace --all-targets -- -D warnings`；Rust format、变更的 TS/脚本
  ESLint/Prettier、docs/public-docs/test-layout 和 diff whitespace 检查。

运行时扩展与 preparation 的既有回归也已通过；存储测试使用临时目录或内存 SQLite，未重置实际
开发数据库，未调用真实厂商模型或变更凭据。独立审查核对了消息/会话/Agent 删除的级联；删除
锚定消息会删除其问答事实，自然结束 Run 不会。Fork 的四张问答事实表归类 RuntimeOnly，
后续用户气泡必须携带自己的展示材料，不能依赖复制活跃批次或投递记录。

第 2 轮从 `HumanInteractionExtension` 的真实 Host 执行服务接入开始：实现调用去重与暂停的
原子绑定、独立恢复来源、用量和停止围栏、启动对账，然后才开放阻塞工具。同步回应须唯一通过
原 ToolResult 进入模型；完成用户气泡投影时，不得同时注入普通 User。异步 handler、投递协调
和空闲续接保持第 3 轮范围。以上为第 1 轮交付时的历史状态；第 2 轮变更见下文。

## 第 2 轮：阻塞式工具

### 执行与恢复

- `AgentHumanInteractionRuntimeHost::suspend` 接受 Runtime 生成的 native `AgentUserInputSuspension`。
  driver 识别内部 `Suspended`，冻结原调用、前序结果、后序队列、Provider continuation 引用、模型上下文、World State 与扩展。
  Host 在同一写事务保存问题、v14 检查点的 allowlist 信封、累计用量及 `waiting_for_user_input`；成功后才通知用户。
- Host 执行 segment 退出并释放执行许可、模型调用与引导队列。Conversation 的 in-progress trace 和逻辑 Run 身份继续占用聊天。
  根聊天不能借此开始新 Run、切换模型、手动压缩或分支。子 Agent 和无人值守 Automation 没有同步工具或 native 执行入口。
- 提交仅形成一条不可变 Response。Host 在提交完成、旧 segment 退出及启动时事件驱动扫描待投递事实，不轮询人类。
  领取使用持久 claimId，并在进入 Runtime 前执行 `claimed → executing` CAS。可信 `AgentUserInputResume`
  带 requestId、responseId、原 checkpoint 与精确 continuation；没有 ApprovalDecision，也不授予权限。
- 完成的工具不重跑，答案只生成原调用的一个 ToolResult，剩余工具按原顺序继续。
  同响应中的多个同步调用顺序暂停。同步后出现真实审批，由现有审批路径处理。
  sync→sync 和 approval→sync 交接将新检查点与前记录结算放入同一事务；sync→approval
  保存审批检查点后确认消费，重启能由该精确检查点核验交接。
- 关闭提问设置不撤销既有回答。新采样边界重新冻结设置/工具/提示词一致快照；新的提问创建再次校验实时设置。
  异步执行 readiness 继续关闭，任何同步暂停/停止均不取消其他异步批次。

### 用量、重启与停止

逻辑 Run 的 `usage-{runId}` 记录保存累计值。发布问题前合并本 segment 一次；waiting State/Done 及旧 segment 收尾只读取累计值。
重启从原 Usage 行恢复冻结价格、Provider 用量语义和累计值，不借用当前模型价格。正式答复即使全部 skipped 也恢复原调用。

同版本 `waiting` 可继续等待；已回答但未领取可继续投递；`claimed` 且尚未开始执行可重新领取。
`executing` 是进入 Runtime 前的未知副作用围栏，覆盖剩余工具和模型请求；`model_in_flight` 是存储预留的细分状态，当前不据它推断请求已知完成。
启动发现已进入执行但没有可靠后继检查点或终态原结果证明时保守失败，不自动重跑工具或结果未知的模型请求。
已有后继审批/同步检查点或终态中精确匹配的 ToolResult 时按事实结算。停止围栏优先于迟到回答和领取。

恢复信封沿用 `PersistedAgentResumeInput` v11 的显式字段白名单，内部 checkpoint 只接受 v14。
API token、带秘密的 endpoint、不可持久化 MCP 原始参数不进入新表；恢复时校验冻结 Provider 身份后才重新读取凭据。
文件事务和 MCP 的已有安全约束继续有效：若剩余工具无法通过现有安全检查点规则保存，问题发布前拒绝本次暂停，返回失败工具结果，不存原始私有参数。
未完成的文件事务保持原有只允许对应提交工具的边界。

### 问答展示、历史、压缩与分支

同步不创建 `messages.role=user` 行。唯一原 ToolResult 的 `result` 使用固定投影：

```json
{
  "type": "human_interaction_response",
  "schemaVersion": 1,
  "requestId": "Host request ID",
  "responseId": "Host response ID",
  "answers": [
    {
      "questionId": "Host question ID",
      "question": "冻结问题",
      "kind": "text",
      "answer": "用户回答"
    }
  ]
}
```

`option` 包含 `optionId` 和解析后的选项文字；`skipped` 的 answer 为“已跳过”。题目顺序来自不可变 Request。
这个事实随原 ToolResult 的 Trace/ModelContext 保存，第 4 轮以用户外观气泡渲染它；不把展示结果再次拼入模型 User 历史。
历史恢复和压缩沿用原工具交换，分支只复制边界内历史 ToolResult，不复制问题的活动权限、suspension 或 delivery。
一般大小与文本合法性约束继续生效，不以产品题数上限截断已正式回答的问题。

### 第 3 轮接入点

实现 `request_user_input_async` 的独立动态 readiness、原子批次接纳及投递协调。共用已有 Request/Response/CAS 和设置策略。
运行中答复须选择可引导边界；审批和同步等待持有聊天占用时保留 pending 回应，真正空闲时才创建同聊天新 Run。
明确异步每批忽略无 User/guidance/Wake；不能复用同步的“原 ToolResult 恢复”途径，也不能在 submit RPC 中直接绕过占用 startTurn。
多批次前端抢占、分页草稿、最小化与气泡渲染仍留在第 4 轮，本轮不实现这些页面。

### 第 2 轮验证记录

已实际执行并通过以下检查。筛选范围有交集，不能直接相加作为独立测试总数：

- `cargo test --locked -p mycopilot-core human_interaction --lib`：56 项；含真实 Harness 顺序执行、
  连续暂停、真实审批交替、根/子/Automation 隔离、实时关闭设置，以及完整有界问答投影。
- `cargo test --locked -p mycopilot-core-server application::agent::tests::human_input --bin core-server -- --nocapture`：10 项。
  可控本地 Provider 经过真实 Harness/Host：混合回答、全部 skipped、重复提交、同 Run 连续提问、
  提问→命令审批→提问、等待跨重启、回答已保存但未领取跨重启、等待 Stop、重启后 Stop，
  以及两个明确调度钩子的快速回答/旧 segment 迟到收尾和 open→submitted 通知顺序竞态。
  验证原调用唯一 ToolResult、无额外 User 行或答案 User 投影、原 Run 身份及精确累计用量。
- Rust Core `runtime::checkpoint`：40 项；`approval_resume`：2 项，覆盖冻结 Provider 身份、队列顺序、
  文件观察证明、私有 MCP 参数拒绝/脱敏和加密恢复。
- 存储问答 repository：27 项（含 11 项同步事务/重启/领取状态测试）；migration：30 项；
  `storage-reset-dev`：58 项；trace reconciliation：11 项。未知执行阶段不重放，停止/CAS 与交接事务保持原子。
- Core Server 既有 `usage_lifecycle`：13 项；`cancellation`：9 项；`terminal_events`：29 项。
- TypeScript 相关单元测试：首批 7 文件 162 项，追加回归 5 文件 34 项；浏览器 5 项。
  覆盖新状态的解析、等待占用、用量、重读/重启、停止、禁止新消息/压缩/fork，及迟到审批 RPC 不覆盖新等待状态。
- `pnpm typecheck:node`、`pnpm typecheck:web`、变更 TS/脚本 ESLint、Prettier、
  `cargo clippy --locked --workspace --all-targets -- -D warnings`、Rust 格式、
  `check:docs`、`check:public-docs`、`check:test-layout` 与 `git diff --check` 均通过。

测试使用临时 SQLite 与可控本地 Provider，未调用真实付费模型、未重置实际开发数据库或改写凭据。
数据库采用 canonical v37，fingerprint 为 `sha256:3f9d66722cd1e6c7ee26512a166a906fa8471a05083522d084ba2182b8fc4a36`。
旧库通过既有受管开发 reset 流程处理，不提供旧聊天迁移。

本轮已结束。没有接入异步投递、设置页或答题面板；第 3 轮从上述异步接入点继续。
结果未知的执行按持久围栏保守失败，公开 delivery 标记失败/取消而不伪称已投递。普通上下文大小和私有工具检查点约束仍适用。

## 第 3 轮：异步工具与回答投递

### 接纳与提示词

`AgentHumanInteractionRuntimeHost::accept_async` 接收可信的 `AgentAsyncUserInputRequest`。
Host 校验当前执行 segment、根节点、Run/assistant/toolCall 和实时设置，在写事务中创建独立批次。
同一原调用只能创建一次；成功后返回 requestId，Harness 立即生成一个普通工具结果：

```json
{
  "type": "human_interaction_accepted",
  "schemaVersion": 1,
  "requestId": "Host request ID",
  "status": "accepted"
}
```

该结果正常完成原 ToolCall。回答不会再次补 ToolResult，也不让该工具 Future 等待人类。
原工具调用的模型用量继续按普通 segment 结算；回答记录、入队和显示本身不产生模型用量。

同步和异步 readiness 独立；`human.interaction` 与 `human.interaction.async` 动态能力共享一次
设置快照，工具 Schema、RequestOnly 提示词及能力状态保持一致，稳定前缀不变。
专项说明要求同步用于必须等待用户参与的工作，异步只能继续不依赖回应的工作，不重复请求。
请求内容可包含信息、偏好、决策、反馈或人工操作；选项不使用固定状态模板，回应解释与验证保持事实边界，交互不能替代权限审批。
`natural_sampling_state` 只在本来就要发生的合法模型采样边界读取已忽略 requestId；它不创建持久
User/guidance、队列消息或 Wake，不要求 Provider 为忽略状态再推理一次。各 Provider 沿用现有
RuntimeGuard 的线协议角色转换；内部状态仍是 RequestOnly 运行信息，不是正式用户回答。

### Host 仲裁与持久化路径

`submit` 先提交不可变 Response 和 pending Delivery，再调用
`AgentService::schedule_human_input_deliveries`。同步恢复优先，异步按 Response.sequence 扫描。
Host 的 dispatcher 串行选择路径；SQLite 的 revision/CAS、停止范围和逻辑 Turn 占用是最终依据。

| 场景                        | 路径与结算点                                                                                            |
| --------------------------- | ------------------------------------------------------------------------------------------------------- |
| 合法活跃 segment 可接收引导 | 同事务绑定 Response、目标 Run 与 guidance journal，然后放入现有 steering 队列                           |
| 完整工具批次结束            | Runtime 按队列顺序加入 UserGuidance；Trace、ModelContext、guidance applied 与 Delivery applied 原子提交 |
| 审批或同步等待              | 先保存 pending 回答；合法恢复 segment 注册队列后接入，仍等完整工具批次结束                              |
| 手动压缩或厂商切换占用      | 保持 pending；操作释放占用后由 Host 再次调度                                                            |
| 同聊天真正空闲              | 复用正常 HumanRoot Turn 准备与权限路径；User、assistant、Run lease、回答绑定和回执在同事务保存          |
| 新回答 Run 执行             | 进入 Runtime 前持久 executing 围栏；绑定目标的已完成 agent_loop 模型请求事实确认 applied                |

初始 Run 的预排队引导也在第一次合法采样前应用；审批或同步恢复必须先完成被冻结的剩余工具队列。
队列在终结或暂停时关闭，尚未应用的异步 guidance 可以回到 pending。重新绑定使用新的 guidance
及 clientMessageId，避免旧 rejected journal 与同逻辑 Run 恢复冲突；正式回应身份始终是 responseId。
如果旧队列已关闭但旧 Run 还没有持久终结，Host 不会把“无 worker”当作空闲另开 Run。

运行中只投影 UserGuidance，不另插 messages User；空闲续接只插一次普通 User，不另投 guidance。
两者携带相同 `HumanInteractionResponseDisplay` JSON。同步依然只投原 ToolResult。
完整有界问答材料不会被通用 guidance 的 12000 字符限额截断；普通 guidance 的限额不变。

调度由提交、segment 启动/恢复、Run 释放、压缩结束和启动恢复驱动，不轮询等待人类的问题。
暂时不能投递时答案保持 submitted，原问题不能重开。新 Run 准备从当前聊天模型和可信持久权限
构造输入，不能借答案继承无人值守身份、子 Agent 身份或提升权限；模型配置和凭据仍由原路径解析。

### 停止、重启及未知执行

每次正式提交在同事务冻结当时占用聊天的 stopScopeRunId。显式 Stop 取消此前接纳、尚未应用且
属于该停止范围或目标 Run 的回答，并关闭对应排队 guidance；迟到队列、结束回调和重启不能
把它们重新投递。自然完成和 Stop 都不撤销仍 open 的异步批次；Stop 后用户新的明确提交是新输入。
关闭提问设置同样不影响既有问题结算及回答投递。

同版本重启先对账 guidance，再恢复问答事实：

- 未提交问题保持 open，已提交 pending 按原 Response.sequence 继续仲裁。
- 未跨越持久应用边界的 queued guidance 可安全重新选择路径，applied guidance 由模型历史继续承载，不能再次入队。
- 新回答 Run 已绑定但尚未执行时，确认没有模型请求或 Trace 副作用后终结空 assistant，撤下未消费的 provisional User，
  保留其稳定消息 ID，回答恢复 pending。后续重建相同 ID，防止未消费的 User 被普通下一轮历史提前读取。
- executing 后没有可靠模型消费证明属于结果未知：Delivery 标 failed，禁止盲目重放模型或工具。
  applied、failed、cancelled 均不会因重启回到可编辑问题。原回答事实始终保持 immutable。

非阻塞 `ignore` 与 submit 共用 revision/CAS 和 publication 锁，仅结束指定批次；不创建 Delivery，
不调用 dispatcher，不发送草稿或用户输入。全部 skipped 后 submit 仍走完整正式回应路径。

### 第 4 轮前端接续契约

本轮不添加答题页面。前端通过已有 Host API 实现产品契约，不能直接选择 Run 或模拟工具结果：

1. `listRequests` 与 `requestChanged` 提供完整批次；分页按 Request.sequence 倒序。提交顺序由 Host Response.sequence 决定。
2. 使用 Host 的 request/question/option ID，提交 `expectedRevision`、稳定 `submissionId` 和完整答案联合数组。
   网络重试复用原 submissionId 与同一 payload；不能生成新身份反复提交。
3. submit 返回 submitted 即关闭卡片及该批入口；delivery 仍可能 pending/bound。相同 request revision 下 delivery revision
   可以继续增长，Renderer 应独立比较两者，不得忽略较新的投递快照或据此恢复编辑。
4. 公共 `HumanInteractionResponseDisplay` 和 `parseHumanInteractionResponseDisplay` 提供问题＋答案展示结构。
   从可信 Request/Response 事实按题序构造即时用户气泡，再与同步 ToolResult、活跃 UserGuidance 或普通 UserMessage
   的 responseId 合并为一个展示；不能仅凭任意普通消息包含 JSON 标记就给予问答权限。
   历史/分支可以直接渲染冻结完整材料，但没有重新回答入口或投递动作。
5. active 路径 delivery.targetRunId 指向被引导 Run，userMessageId 为空；idle 路径两者都存在。
   收到新的目标绑定后重读同聊天消息和 Run 状态，载入 Host 自动创建的普通 User/assistant；通知丢失时从查询重建。
6. `ignored` 不展示正式回答气泡；`submitted` 且所有 skipped 显示逐题“已跳过”。最小化、翻页和草稿只在前端保存，
   不调用 ignore/submit；审批抢占不能丢草稿。关闭设置后已有批次继续可提交/忽略。
7. 文本框 Enter 和 Escape 不得隐式提交、跳过或取消运行；中文输入法的组合确认只作用于输入框。
   所有分页、优先级、灰色问题文本及审批框视觉复用按前述完整产品契约完成。

### 第 3 轮实际验证

以下检查已执行通过，筛选范围部分重叠，不相加为独立总数：

- `cargo test --locked -p mycopilot-core-server application::agent::tests::human_input --bin core-server`：27 项，
  其中第 2 轮同步 10 项和第 3 轮异步/共享协议 17 项。可控本地 Provider 经真实 Host/Harness 验证：
  多批次按提交顺序、active→idle、自然结束后回答、全部 skipped、ignore 无 Wake、同版本重启、
  审批/同步等待及旧 guidance 重绑、真实手动压缩占用结束自动投递、Stop 前后竞争、
  失败首样和审批续跑前失败后的结算/后续投递、同一答案唯一历史投影、精确模型请求数与用量。
- Rust Core `runtime::tests::human_interaction`：9 项；`runtime::preparation::human_interaction_tests`：9 项；
  `runtime::extensions::human_interaction`：6 项；`conversation_trace_projection` 中问答投影专项：4 项；
  `runtime::tests::steering_and_repair`：6 项。包含真实 Provider 的 accepted 后续工具顺序、async→sync
  冻结队列→恢复后 guidance、自然 ignored 状态、根/所有子级/Automation 隔离、动态快照及长答复不截断。
- `cargo test --locked -p mycopilot-core human_interaction_repository --lib`：36 项（包含 9 项新增存储验证）；
  `storage::migrations::tests`：30 项；`storage-reset-dev`：59 项；fork 表分类专项：1 项。
  验证 CAS/幂等、submit 与 Stop 并发、Trace/ModelContext 与回执同事务、绑定前启动失败回滚、
  未执行/执行未知/完成消费三种重启情况、停止围栏及 exact v37 配置保留 reset。
- Core Server 既有 `application::agent::tests::steering`：13 项；`application::agent::tests::pending_actions`：71 项。
  后者共享 fixture 曾遗漏第 2 轮新增的必填 `pauseReason`，已更新为当前协议的 `approval` 后全部通过；
  没有降低生产校验或加入旧检查点兼容。
- TypeScript：协议/display/Main RPC 专项共 35 项，含跨语言 fixture 和约 480 KiB 的完整 queued/applied 问答内容；
  `pnpm typecheck:node`、`pnpm typecheck:web` 及变更 TS/脚本 ESLint 通过。
- `cargo clippy --locked --workspace --all-targets -- -D warnings`、Rust/Prettier 格式、
  `pnpm check:docs`、`pnpm check:public-docs`、`pnpm check:test-layout` 及 `git diff --check` 通过。

独立审查发现并已修复：首个模型请求失败后 receipt 长期 bound、审批续跑在 Runtime 前失败遗漏调度、
以及新增取消续跑调度可能重入 deletion lock。相应终态出口现在先完成持久结算并释放锁，再调度合法待投递回应。

Canonical SQLite 为 v38，fingerprint 为 `sha256:03332e3251b0660eeff21c26300cec499009aee56b68d3d92556a8c22f047a67`。
本轮使用临时数据库和可控本地 Provider，未清空实际开发数据库，未调用付费模型或改写 API 凭据。
没有旧聊天/旧运行/旧检查点迁移；只更新当前 schema 和现有开发 reset 配置保留路径。

第 3 轮已结束。结果未知的执行保守标记失败，不自动重放；前端页面、分页草稿、最小化与视觉渲染留待第 4 轮，
按上述完整 Host 契约接入，不由 Renderer 新建或选择回答目标 Run。

## 第 4 轮交付：前端与 Host/Preload

### 设置与独立问题状态

[`HumanInteractionSettingsSection.tsx`](../../src/renderer/src/features/settings/pages/HumanInteractionSettingsSection.tsx)
直接挂在个性化页，使用独立 get/updateSettings、expectedRevision 和通知；读取确认前不伪造设置值。
保存过程中保留已确认值，保存失败显示错误，成功才应用较新的后端 revision；其他个性化内容仍走原保存流程。
九语种文案在独立 humanInteraction 翻译片段维护。关闭设置不参与待答问题列表的过滤。

[`humanInteractionController.ts`](../../src/renderer/src/features/humanInteraction/humanInteractionController.ts)
与 [`useHumanInteraction.ts`](../../src/renderer/src/features/humanInteraction/useHumanInteraction.ts)
独立于当前 Run 事件读取所有分页。每批保存页码、互斥答案草稿、操作状态和稳定提交身份；组件隐藏、抢占、
最小化及切换聊天不销毁草稿。未提交草稿属于当前窗口的内存状态；完整问题与已提交回答由 Host 持久化。

Request revision 与 Delivery revision 分别单调合并，终态和已删除批次保留墓碑。
整页扫描必须全部成功后才能剔除缺失项；扫描期间到达的新通知不会被迟到页覆盖。
提交/忽略失败保留草稿，结果未知时冻结原 payload 并重用 submissionId，不能通过改答案或另一种操作绕过未确认的请求。
确认拒绝后的未改动 payload 重试同样保留身份；用户修改草稿才创建新身份。

Main 将 Core Server 启动/重连生命周期转为独立 `humanInteraction.onResync`；Preload 继续校验快照，
问题和设置通过自己的查询恢复。窗口 focus、online、可见性恢复也会重读问题。审批结束后必须先成功重读权威问题状态，
才能恢复答题操作；失败显示重试入口，不直接解锁旧缓存。后端事实始终优先于本窗口的进行中操作。

### 面板、抢占与历史展示

[`HumanInteractionPanel.tsx`](../../src/renderer/src/features/humanInteraction/HumanInteractionPanel.tsx)
是纯受控展示组件，不调用模型或伪造 Host 成功。字号、边框、颜色、圆角、间距与当前审批框变量一致，
没有修改审批框原样式。长问题和选项在面板内容区滚动，页数不做产品上限。

顶部箭头翻页；异步右上角是最小化；底部异步左侧“忽略全部”，右侧“跳过／已跳过”和“下一题／提交”。
三种答案互斥，整批完整时才切换为提交；翻页不发送答案。自定义回答为同行单行输入，Enter 不提交，IME 正常确认，Escape 不上冒到
停止/审批处理。进行中的结算禁用重复动作，未知结果保持原答案用于安全重试。

[`ConversationSurface.tsx`](../../src/renderer/src/features/chat/ConversationSurface.tsx)
协调根审批和协作审批 > 同步提问 > 异步提问。审批独占时所有问题入口不可操作；同步提问也阻止异步入口抢占。
异步按创建 sequence 后来者优先，手动打开旧批次只改变面板选择；提交顺序仍完全由 Host 接纳序号决定。
每批在所属 assistant 原工具调用位置有一个“交互 · 共 N 题”入口，位于调用前后正文之间，折叠运行详情不改变顺序；成功提交或忽略后入口立即撤下。
子 Agent observer 不查询待答批次、不展示操作入口。

[`humanInteractionPresentation.ts`](../../src/renderer/src/features/humanInteraction/humanInteractionPresentation.ts)
只产生渲染用副本，不写 messages、Trace、模型上下文或新增 Run：

- 同步原 ToolResult 在其时间线位置显示为用户气泡，保存的原历史仍只有工具结果。
- 异步运行中使用 Host 专属 guidance 身份；运行后使用 delivery.userMessageId 精确绑定的 User 行。
- 刚提交的权威 Request/Response 可以即时展示，再按 responseId 与后到的 ToolResult/guidance/User 合并。
- 历史分支的冻结材料只提供只读展示，不能派生待答问题权限。普通用户碰巧输入同结构 JSON 时不隐藏消息、
  不参与问答去重，也不把普通 guidance 错当成 Host 回答丢弃。
- 问题使用次要文字色，答案使用正文色，skipped 本地化为“已跳过”；没有完成卡片、补答或修改答案入口。

Host 回答被拒收或需要重新选择投递路径时，不进入原 Composer 的引导恢复/自动排队发送流程。
主 Composer 继续保留普通输入、附件与排队逻辑；Host 自动创建回答 User 行不触发清空普通草稿的消息同步键。

[`useHumanInteractionConversationSync.ts`](../../src/renderer/src/app/useHumanInteractionConversationSync.ts)
在独立通知和重连后载入同聊天的权威消息及 Run 身份；沿用现有 Run 绑定与事件回放，Renderer 从不 startTurn。
读取串行合并，迟到快照不能回退已停止状态或较新的 trace/流式内容；本地 UI 状态独立保留，不能屏蔽后端完成状态。

### 第 4 轮验证与第 5 轮入口

本轮使用浏览器中的真实组件、受控 Host 契约 fixture，以及临时数据库和可控 Provider 的真实 Host/Harness 回归。
生产设置、列表、提交、忽略、重连均调用已有 Host/Preload/Core Server RPC，没有纯前端成功分支。
未重置实际开发数据库、未改写模型配置/API 凭据，没有数据库迁移或 schema 变更，当前仍为 v38。

实际执行并通过的检查（筛选范围有重叠，不应相加为独立总数）：

- Browser：8 文件、51 项。包括面板 6 项、独立状态 hook 6 项、个性化设置 9 项、真实 ConversationSurface/Composer 集成 6 项、
  会话同步及真实 Run lifecycle 8 项；其余为已有审批密度、Guidance 和 observer 回归。
  验证分页修改、70 题、IME/Enter/Escape、busy/unknown 重试、审批抢占恢复、后来批次优先、最小化、
  每批唯一入口、提交后消失与单气泡、普通 JSON 与普通草稿不受影响、重连和旧审批列表迟到。
- TypeScript unit：7 文件、43 项，覆盖独立控制器、展示去重/分支冻结材料、会话快照合并、公共协议、Main RPC/IPC 与 Preload。
  另执行语言完整性 6 项，确认九语种新增文案齐全。
- `cargo test --locked -p mycopilot-core-server application::agent::tests::human_input --bin core-server`：27 项通过，
  沿用真实 Host/Harness、临时 SQLite 和可控 Provider 检查同步恢复、异步多批次、暂停/停止/重启与唯一投递及用量。
- `pnpm typecheck`、全部变更 TS/脚本 ESLint、Prettier、`pnpm check:docs`、`pnpm check:public-docs`、
  `pnpm check:test-layout` 及 `git diff --check` 通过。

扩展回归中 `AppShellSkillRecovery.browser.test.tsx` 的 91 项通过。
既有 `AppShellCollaborationScenario.browser.test.tsx` 有 1 项截图稳定性失败、1 项通过：同次连续截图从 333×931 变为 333×936。
在未修改的 HEAD `79ec7341bf63cb5b4dbac218014c61326c6551c5` 独立 checkout 中，同一断言复现，
两张 PNG 分别与当前工作区失败输出具有完全相同的 SHA-256，因此记录为已有问题；没有修改其测试断言或审批样式。
临时 checkout 已清理，测试覆盖写入的既有截图已恢复。此项不计入上述通过的本轮专项检查。

本轮已停止于前端交付，没有执行打包 Electron 的全套人工验收。以下是进入第 5 轮时的待验收清单，最终结果见文末：

1. 打包 Electron 窗口中的完整父智能体工具调用→审批/提问交替→提交→正常后续运行；跨 Provider 的工具与专项提示词动态一致。
2. 多窗口、Core Server 重启和客户端重开组合验收：未答批次、已提交未投递、恢复领取中、Stop 围栏与删除竞争。
3. 同步/异步答案经过压缩、latest fork、重启历史恢复后仍唯一进入模型；独立用量及 Run 用量不重复。
4. 子 Agent 所有层级和无人值守任务继续无提问能力；关闭开关后旧问题仍可结算。
5. 整体验收发现的问题修整，更新面向用户的操作说明和最终验收矩阵。

## 第 5 轮：最终验收与修复

本轮从实际代码和测试重新验收，沿用前四轮产品契约。全部验证使用临时数据库、本地可控 Provider 或测试专用 transport；未打开或重置实际开发聊天库，也未修改真实模型配置或 API 凭据。

### 实际修复

1. **同步恢复领取失效后的无 worker 接收队列**：将活跃控制注册移动到所有可失败准备和持久执行 CAS 成功后。准备失败且已确认原 Run 持久终结时，调度其他已经保存的异步回答，避免答案被遗留在无 worker 的队列。可控断点在修复前明确复现失败，修复后覆盖领取撤销和真实 Stop。
2. **问答历史的可信展示来源**：新增纯历史 `human_interaction_message_projections`。空闲异步答案 User 与证明同事务写入；消息读取返回只读 `humanInteractionResponse`，内容必须与原 User JSON 一致。Main/Preload 拒绝在消息保存接口提交证明，native 写入也不能授予该来源。普通用户碰巧输入问答 JSON、通用历史 snapshot、伪造元数据均不能成为正式答复。
3. **分支保留权威文字与来源**：仅复制可见消息对应的纯历史证明并重映射外围消息身份，不复制待答问题、投递、挂起、权限或用量。完整冻结问答对象不参与通用字符串 ID 替换，答案恰好等于旧 Run/message/call ID 时仍保持原文。
4. **恢复和前端迟到状态**：独立重连查询成功后再恢复操作；Host 切换和新批次抢占后的旧回调不能改写已失去控制权的批次。Host 创建 User 已到达而投递回执尚未到达时，仍以可信历史证明渲染并保留普通 Composer 草稿、附件。
5. **保留回答标识与子历史继承**：公开 `steerRun` 拒绝 `human-answer-` 保留前缀，拒绝时不发布可被误认成正式回答的 Guidance 事件。内部回答仍经可信投递路径正常入队；所有子 Agent 的上下文快照只复制已验证的历史证明，不复制提问权限。
6. **同一回答的恢复证明**：同步消费证明同时核对原调用的 trace、模型上下文和完整问答结果；仅有相似工具名或缺少正文对应关系不算已投递。

Canonical schema 升为 **v39**，fingerprint 为 `sha256:993ab20442c1e258798e8d07bec6922cc46d11bf6a633ceb3cac2a6b80b23078`。
开发 reset 支持 exact v38 配置提取到全新 v39，保留模型/凭据引用和独立人机交互设置，丢弃旧聊天/运行/问答。没有旧聊天迁移和旧检查点兼容链路。

### 验收分层与实际边界

| 范围                        | 实际验证入口                                                              | 覆盖的关键条件                                                                                                                                                                    |
| --------------------------- | ------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Harness 工具与能力          | `runtime::tests::human_interaction`                                       | 同请求 schema/提示词/能力一致；开关开关再开；普通根可用，直接子、深层子、拥有后代的子及 Automation 均拒绝                                                                         |
| Core Server 与真实 Provider | `application::agent::tests::human_input`                                  | 同步唯一 ToolResult/原 Run/有序剩余工具；连续同步与真实审批交替；快速回答/停止/重启/领取竞态；异步多批、safe steering、active→idle、忽略与独立用量                                |
| Storage/历史                | `human_interaction_repository`、fork/context/reset 专项及 workspace tests | CAS/幂等、完整答案证明、历史分支和压缩、源删除/嵌套分支、新库与 exact 旧开发配置 reset                                                                                            |
| 真实跨进程 UI 链路          | `pnpm test:human-interaction-core-e2e`                                    | locked Chromium 的生产面板与 Preload → Main RPC → 真正 stdio Core Server/Harness → 本地 HTTP Provider；浏览器同步提交、动态关闭、多异步批次、重启、忽略、空闲续接及双窗口撤下入口 |
| Renderer                    | 问答、审批、ConversationSurface、会话同步、observer browser tests         | 分页和草稿、IME/Enter/Escape、提交禁用、审批独占恢复、同步优先/后来优先、最小化/唯一入口、单气泡、乱序通知、Composer 与观察模式                                                   |
| Host 与协议                 | TypeScript unit、Rust protocol、Main/Preload tests                        | 输入联合类型、只读证明、防伪、通知/查询边界、存储序列化、重复结算与设置 revision                                                                                                  |

新增跨层专项使用生产 Preload bridge 和 Main registrar，只适配 Electron IPC 传输；问答结果全部来自真实 Rust Core，不用页面回调假成功。该专项没有启动打包 Electron、原生 contextBridge 或 OS 窗口，不能把它计为打包应用人工测试。已接入 CI macOS job，独立命令不在 `pnpm check` 聚合中。

审批恢复和 MCP expiry 旧测试夹具缺少当前 v14 必填 `pauseReason` 的 9 项失败，本轮补齐当前版本夹具；生产校验保持严格。新历史证明增加一个按聊天批量读取的固定查询，恒定查询数量回归同步更新，未引入按消息循环查库。

### 使用与恢复限制

- 未提交草稿和页码只保留在当前窗口内存；最小化、批次抢占、聊天切换保留，关闭窗口/完整重载不恢复草稿。未答问题与已经提交的回应由 Host 持久化。
- 每批没有题数产品上限，仍有通用 JSON、文本长度和内容合法性上限；本阶段不支持答题附件和提交后修改。
- Provider/工具已进入执行但结局未知时保守失败，不重放可能收费或有副作用的工作。已提交回答保持不可变；用户需在核对实际状态后明确继续。
- Stop 围栏阻止此前已接纳回应的迟到续跑；用户在 Stop 后明确提交仍 open 的异步批次是新的输入。
- 只有根聊天有工具；所有子 Agent 和无人值守定时运行没有提问权限。问答不代替权限审批。
- 本轮没有商业 Provider 联网验收、真实 macOS 输入法候选窗人工操作、OS 休眠唤醒或打包 Electron 全套人工测试；可控 Provider 和各层生产链路覆盖不能外推为所有服务商版本均已实测。

用户操作说明见[回答智能体的问题](../../public-docs/user/everyday-use/answering-questions.md)、[设置参考](../../public-docs/user/reference/settings.md)与[状态说明](../../public-docs/user/reference/statuses.md)。开发验证与恢复说明同步更新于[测试策略](../development/testing.md)和[恢复 Runbook](../operations/recovery-runbook.md)。

### 最终执行记录

最终执行结果如下，筛选运行与全量运行有重叠，不能相加为独立测试总数：

- `cargo test --locked --workspace`：退出码 0；标准 Rust 测试报告合计 **3,545 passed / 0 failed / 14 ignored**。包括 Rust Core 2,456、Core Server 847、开发 reset 60、Rust 协议 37，另有集成及 doctests。14 项仍按现有 ignored registry 属于独立/组件/压力专项，本轮没有执行，不能计为通过。
- 核心定向验证包括真实 Provider 的同步/异步 Host/Harness 31 项、动态能力/身份 11 项、Observer 展示 1 项；新增 Host 保留前缀拒绝与子继承纯历史测试也在最终 workspace 内通过。新 query-count、源删除后二次 fork、压缩前完整问答来源和 exact v38 reset 均已进入最终测试。
- `pnpm test:unit`：**204 文件，1,779 项通过**。
- 受管 Chromium browser 专项：**9 文件，147 项通过**。涵盖问答 controller/panel、设置、真实 ConversationSurface 与 Composer、会话同步、ApprovalDensity、Guidance、observer 和 AppShellSkillRecovery。组合输入事件不等于真实 macOS 输入法候选窗人工验收。
- `pnpm exec vitest run --project human-interaction-core-e2e`：**1 文件，2 项通过**。真实 stdio Core Server 与临时 SQLite 经生产 Preload/Main 到浏览器；受控模型请求确认原 ToolResult/下一轮唯一答案、关闭设置、保留前缀防伪、两批异步、重启后恢复、忽略无请求、一次后续 Run 和另一窗口结算。Vite 使用独立临时 cache，最终 browser 复跑无测试中途重载。
- `pnpm typecheck`、`pnpm lint`、`pnpm format:check`、workspace all-targets Clippy `-D warnings`、`pnpm check:docs`、`pnpm check:public-docs`、`pnpm check:test-layout`、`pnpm verify:agent-avatars`、`git diff --check`：全部通过。
- `pnpm test:test-infrastructure`：10 项通过；`pnpm test:storage-reset-dev` 脚本层：10 项通过。实际用户数据根的 reset 没有执行。

执行中出现的失败均区分记录：首次纯历史查询数由 13 增为固定 14、当前检查点夹具遗漏 pauseReason、构建中尚未完成的 DTO 字段/类型及专项测试定位符在收敛后均修复，最终上述范围全部通过。Clippy 的新测试写法也已修整通过。

没有声称执行完整 `pnpm check` 聚合、全量 browser 或原生 Electron suite。第 4 轮记录的既有 AppShellCollaborationScenario 截图稳定性问题本轮未重跑/修复；本轮执行的是与问答、审批交互和改动范围匹配的 browser 专项。打包应用、系统 IME、商业 Provider、OS 休眠与 ignored 专项保持上文列出的未验证边界。

五轮开发与本轮必需验收已完成。旧 v38 开发库需退出应用后使用受管开发 reset 新建 v39；先运行 `pnpm storage:reset-dev` 阅读只读预检，再按确认步骤操作。模型配置和凭据引用保留，旧聊天/运行/问答不迁移。没有自动执行此破坏性数据重建，也没有改动其他任务的审批样式或既有截图。

## 验收后界面调整（2026-09-05）

- 交互面板替换 Composer 的可见位置。Composer 保持挂载并在交互/审批占用时 hidden/inert，保留消息快路径草稿和附件；非阻塞最小化后恢复，短窗口中交互区可滚动。
- 标题统一“交互”，标题及每批 timeline 入口共用气泡问号图标。面板使用审批视觉变量，缩小行高、间距，自定义回答采用同行单行输入。
- “跳过”仅标记当前题，之后显示“已跳过”，保持深色。当前题有效时“下一题”定位下一未处理题；整批有效时才变为“提交”，一次提交整批语义不变。
- 助手正文、普通用户正文和问答答案使用主题 text.strong；问题继续使用 text.secondary，时间等辅助信息保持原层级。
- 输入区设置移除保存中的提示和灰色说明文字；独立 revision、即时保存、防重复和错误反馈保留。

本次调整验证：7 个问答/审批/观察模式浏览器文件 53 项通过；2 个 Composer 命令/Skills 浏览器文件 41 项通过；问答投影及控制器单元测试 22 项通过；真实 Chromium → Preload/Main → Rust Core/Core Server 的 2 项用例通过（临时数据库及可控 Provider），包含新分页操作与生产 timeline 入口。类型、定向 ESLint/Prettier、开发/用户文档和 diff whitespace 检查通过。未改变数据库版本或重置实际开发数据。

### 同步回答恢复后的流式顺序修复

同步恢复将回答写入检查点和 trace，但原调用的 ToolResult 不一定先以 live 事件到达 Renderer。此前 Request 已提交而 ToolResult 未到时，展示兜底把用户答案追加在 assistant timeline 末尾；新 delta 加入后就被排在答案前，终态读取工具结果后又恢复正确顺序。现在同一 request/run/assistant/toolCall 绑定下，已接纳的同步答案立即占据原调用的 trace 位置；后到 ToolResult 继续使用相同展示 ID，不移动或重复气泡。此变更只作用于展示副本，不创建模型 User 输入或第二个工具结果。

两个新增单测在修复前复现了“正文 → 续写 → 答案”的错误顺序。修复后相关单元 19 项、浏览器 22 项通过，覆盖回答回执早于/晚于续写、多个 delta、stream commit、done、持久化重载及单气泡；Web 类型和定向格式/ESLint 检查通过。
