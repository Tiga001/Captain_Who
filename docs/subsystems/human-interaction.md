---
status: current
audience: developers
owner: engineering
last_verified: 2026-09-05
---

# 向用户提问：设计与实施进度

本文记录完整产品契约及分轮交付状态。第 1 轮完成基础设施，第 2 轮接入阻塞式提问、同一 Run 恢复与生命周期。异步投递、设置页面和答题面板仍属于后续轮次；不得以成功占位代替未实现链路。

## 产品契约

- `request_user_input` 挂起当前根智能体，整批回答作为原工具调用的唯一结果恢复同一逻辑 Run。
- `request_user_input_async` 接纳后立即返回，继续独立工作；整批回答随后作为用户消息投递。
- 两工具只属于直接面向用户的根智能体。普通单 Agent 聊天属于根聊天；所有层级子 Agent，
  包括自身拥有后代者，均不得挂载或执行。第一阶段不向无人值守 Automation 挂载。
- 设置 → 个性化新增“人机交互”及默认开启的“允许智能体向人类提问”。Host 独立保存设置及
  revision，单次输入中的 Prompt Preferences 不具备覆盖此策略的权限。
- 关闭只阻止新提问，已有问题仍可提交或忽略。创建问题与关闭设置在同一数据库写事务边界裁定先后。
- 每题一页，不设置产品题数上限。允许多个未结束的异步批次，一次工具调用对应一批问题。
  整体请求字节数、字段长度和输入合法性仍有技术限制。
- 每题在选项、非空自由文字、“不回答”之间选择；提交前可以修改，全部处理后统一提交。
- 顶部前后箭头翻页。底部“不回答”取代原“上一题”位置，右侧始终为“提交”，
  条件未满足时置灰，满足时采用消息发送按钮的可点击样式。翻页只更新草稿。
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

后续工具描述必须区分“必须等答案才可继续”和“仍有独立工作可做”；缺失信息能够安全推断时
避免反复提问；沉默、不回答和忽略都不等于同意。问题不能替代正式权限审批。

## 数据与状态

- Settings：全局 enabled/revision/updatedAt，默认 true/0/0，独立于 Prompt Preferences。
- Request：Host 创建 sequence、模式、可信 owner、题目快照、policyRevision、版本与状态。
- Response：一次不可变的整批回应及提交身份。正式回应按原题序归一化；忽略不包含草稿。
- Delivery：与 Response 一对一的回答投递事实，区分 pending/bound/applied/cancelled/failed。
- Suspension：同步恢复所需的私有检查点和绑定，不进入公开快照或普通 IPC。

对应五张表为 `human_interaction_settings`、`human_interaction_requests`、
`human_interaction_responses`、`human_interaction_deliveries`、`human_interaction_suspensions`。
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

Canonical SQLite 版本为 v37。正常打开旧库只返回 reset-required，不改写旧库。
受管 reset 可从受支持的 exact 旧指纹读取配置白名单后新建 v37，丢弃聊天/运行历史，保留模型配置和凭据
引用；exact v36 与 current v37 reset 还保留人机交互设置及 revision。无法安全识别且含配置的旧库拒绝
重置，不能默默用默认值替换配置。既有 exact v33 私有备份配置恢复仍受 fingerprint 限制。

## 代码接续入口

| 层            | 入口                                                                                                                                                                                                         | 本轮职责                                                    |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------- |
| 公共协议      | [`human_interaction.rs`](../../crates/core/src/human_interaction.rs)、[`humanInteraction.ts`](../../packages/protocol/src/humanInteraction.ts)                                                               | 输入、答案、公开快照、校验；共享 fixture 防止跨语言字段漂移 |
| 持久化        | [`human_interaction_repository.rs`](../../crates/core/src/storage/human_interaction_repository.rs)、[`service/human_interaction.rs`](../../crates/core/src/storage/service/human_interaction.rs)             | 原子创建、结算、独立设置、Host 私有挂起材料                 |
| Harness       | [`extensions/human_interaction.rs`](../../crates/core/src/runtime/extensions/human_interaction.rs)、[`tools/human_interaction.rs`](../../crates/core/src/tools/human_interaction.rs)                         | 每请求策略快照、根身份过滤、未就绪能力关闭                  |
| Core Server   | [`application/human_interaction.rs`](../../crates/core-server/src/application/human_interaction.rs)、[`transport/human_interaction_rpc.rs`](../../crates/core-server/src/transport/human_interaction_rpc.rs) | 会话访问校验、独立 API、同步回答领取与恢复                  |
| Electron Host | [`coreServerHumanInteractionApi.ts`](../../src/main/core/coreServerHumanInteractionApi.ts)、[`HumanInteractionIpcBridge.ts`](../../src/preload/HumanInteractionIpcBridge.ts)                                 | RPC/IPC 校验与订阅；前端页面尚未接入                        |

## 五轮进度

| 轮次 | 内容                                                  | 状态                 |
| ---- | ----------------------------------------------------- | -------------------- |
| 1    | 公共协议、独立设置、存储事务、Host 契约、模块挂载基础 | 已完成（2026-09-05） |
| 2    | 阻塞工具、暂停恢复、用量、取消与重启                  | 已完成（2026-09-05） |
| 3    | 异步工具、多批次投递、空闲续接、忽略                  | 未开始               |
| 4    | 个性化开关、分页面板、抢占、最小化与问答气泡          | 未开始               |
| 5    | 跨层验收、修整、开发及用户文档                        | 未开始               |

当前提交接口先保存回答，再由 Host 领取并恢复同步 Run；返回 submitted 不代表已 applied。
异步工具仍未暴露，设置页与答题面板属于第 4 轮；前端不能直接 startTurn 绕过投递协调。

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
