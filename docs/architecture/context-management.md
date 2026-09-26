---
status: current
audience: developers
owner: engineering
last_verified: 2026-09-26
---

# 上下文管理

本文定义主模型上下文的事实来源、组装、计量和压缩。Trace、Exact Archive、历史检索与分叉见[Conversation Trace 与 Exact Archive](./conversation-trace-and-archive.md)；Provider wire 协议见[Agent Runtime 与模型 Provider](./agent-runtime-and-providers.md)；FileChange 的 Observation/投影边界见[FileChange 子系统](../subsystems/file-change.md)。

## 职责边界

上下文管理回答三个问题：

1. 下一次模型请求应看到哪些内容；
2. 这些内容占用多少上下文容量；
3. 超过安全容量时，哪个完整前缀可以被摘要替换。

它不负责保存 Provider 隐藏 reasoning、不把 Renderer timeline 当作历史、不决定文件或 Tool 权限，也不把 Exact Archive 原文无条件塞回主模型。

## 权威事实与派生视图

SQLite 中的 `messages`、`ConversationTurnTrace`、当前 compaction head、Conversation World State 请求日志及当前 Run 已确认状态组成逻辑日志。所有消费者从这些事实派生：

```text
messages + Trace/model-context journal + active compaction head
                         |
                         v
                  ContextAssembler
                         |
                         v
                    ContextFrame
      +------------------+------------------+
      |                  |                  |
      v                  v                  v
 capacity report    model request     context snapshot/UI
      |
      v
 compaction planner -> summary generation -> atomic commit
```

`agent_run_json`、Renderer 事件和 timeline 只用于展示，不得反向重建模型历史。`collaborationFinalResponseBoundary` 记录最终正文开始时的位置，Run 结算时冻结回复内的 `collaborationTimelineActivities`；结算后活动显示在实际 owner 会话的消息之间，不改写旧回复，也不凭 UI 活动创建模型输入。模型协作信息仍来自持久 Mailbox/receipt。Provider payload 必须从 Provider-neutral `LlmMessage` 生成；这适用于所有已注册 Provider。

## 逻辑日志顺序

一个已结算 Turn 按下列逻辑顺序展开：

```text
user message
ContextMaterial: attachment projection + preactivated Skill + initial Run World State
assistant public narration
Tool Call + Tool Result      # 一个不可拆分闭环
...
assistant final message
trace terminal record
```

消息位置和 Trace sequence 决定顺序，不能只按墙钟时间拼接。运行中的 narration 与已闭合 Tool Result 可先持久化；活动 Trace 只允许在尾部暂存一个用于审批/恢复的 open Tool Call，该调用不进入已闭合 model-context 前缀或压缩边界。最终助手消息和 Trace 终态在同一持久边界结算。

pending assistant message 的持久正文为空；“正在思考”等 UI placeholder 不属于逻辑日志。FileChange 的 `apply_patch` durable Tool Call 只保存 body-free operation 与 content/edit digest，不保存完整 content、old/new text 或 Observation ID；精确执行材料属于 Host 私有 action audit/Staged store。成功写后的 successor `fileChangeTarget` 只用于同 Run 的下一次模型 continuation 和私有 checkpoint，不进入普通 Event/Trace/Archive。

用户消息的持久正文保持原样。组装时后端可加入确定性的 `<backend_conversation_timing>` 元数据，记录该用户消息和相邻上一条 assistant 的时间；时间来自 SQLite 毫秒时间戳，并按本机时区渲染为带偏移的 RFC 3339。不要修改 assistant 历史正文来承载内部时间标记。

## ContextFrame 分类

同一计量器对完整帧分组，避免容量判断、UI 和实际发送使用不同口径：

- `fixed`：系统提示、Tool 定义和 Provider 固定开销；
- `durable`：当前摘要及其游标之后的已提交消息/Trace；
- `run_transient`：当前 Run 已产生、但尚未被主模型成功观察和提升的内容；
- `request_only`：当前能力使用说明、Todo、文件事务提示等只属于本次请求的材料；
- `reserved_output`：为模型输出保留的空间。

当前 Run 的 Tool Result 可以先落库，但在包含它的模型请求成功前仍属于 `run_transient`，不能被压缩覆盖。请求成功后，已观察前缀才能提升为 durable baseline。

这些分类描述计量与持久化语义，不直接决定模型请求中的物理位置。当前 Run 的附件、Skill 和 Run World State 仍按运行态计量和保护；它们首次进入模型上下文时同时保存可重放的 `ContextMaterial` 事实。Run 结束后，历史重建保留原有角色、正文、图片引用和因果顺序，以 `HistoricalRunContext` 标识已经结束的运行材料。保存的是观察记录，不是 Skill 激活、工具挂载或审批权限。RequestOnly 指南不会因此固化进历史。

## 组装规则

`ContextFrame` 保留逻辑日志顺序；发送前通过独立的 `model_request_items()` 布局投影组织主模型请求。以下是当前主模型请求的完整逻辑层次：稳定与半稳定前缀 → 可连续继承的历史与当前运行 → 当前请求的即时尾部。可选项不存在时跳过。

```text
模型请求
│
├─ tools：实际工具 Schema
│   ├─ 稳定工具，按名称排序
│   └─ 当前挂载的动态工具，按名称排序
│
└─ messages
    │
    │  稳定与半稳定前缀
    │
    ├─ 1. 稳定 System Prompt
    │      基础行为、稳定工具指导、文件事务固定规则、
    │      用户自定义指令等
    │
    ├─ 2. 初始 Skill 简短目录
    ├─ 3–4. 当前已启用能力的使用指南（RequestOnly）
    │      联网搜索 / 人机交互 / 浏览器 / 协作规则与目录
    │      关闭能力后，对应指南撤下
    │
    ├─ 5. 会话 World State full 基线
    │      工作区、权限、环境、模型选择等
    │      联网搜索、人机交互、浏览器、多智能体设置开关状态
    │
    ├─ 6. 当前有效压缩摘要（若存在）
    │
    │  历史与当前任务
    │
    ├─ 7. 摘要之后的统一历史
    │      历次用户输入、助手输出、完整工具交换
    │      原始附件材料与图片引用
    │      当时的 Skill 完整说明、Run 状态
    │      World State diff、忽略提问等状态事实
    │      均保留原有因果顺序
    │
    ├─ 8. 本次用户输入
    │      连续用户消息、其间关联的状态 diff
    │      附件说明与图片
    │
    ├─ 9. Run 开始时预激活的 Skill 完整说明
    │
    ├─ 10. 初始 Run World State
    │       本轮激活的 Skill
    │       浏览器在当前任务中的激活状态
    │       附件等必要运行状态
    │
    ├─ 11. 当前 Run 的因果时间线
    │       助手输出
    │       Tool Call + Tool Result
    │       用户回答、运行引导
    │       运行中新激活的 Skill
    │       会话及 Run 状态 diff、必要恢复快照
    │       忽略提问等普通后端状态事实
    │
    │  当前请求的即时尾部
    │
    ├─ 12. 当前 Todo 的精简提醒
    │       完整计划独立保存，提醒受 500-token 预算约束
    │
    ├─ 13. 必要的单次修复提示
    │       如空响应修复、文件事务纯文字违规提醒
    │
    └─ 14. 未完成文件事务的最小快照
            事务 ID、路径、操作、策略、状态
            expectedDraftRevision、nextIndex
            allowedNextActions

            没有未完成事务 → 整段省略
            已完成事务结果 → 留在正常工具历史中
```

这 14 项是逻辑分区，不代表固定发送 14 条消息，也不把所有内容改成 System 角色。原有消息角色、图片、Tool Call/Tool Result 配对和 Provider 续接格式继续保留。`tools` 独立于消息序列；输出预留预算参与容量判断，不是一条发送给模型的消息。具体排序以[请求布局](../../crates/core/src/context/frame/request_layout.rs)和[工具集排序](../../crates/core/src/tools/tool_set.rs)为准，公开阅读版本见[上下文管理](../../public-docs/user/learn/context-management.md)。

当前布局将初始目录与能力指南放在 Conversation full 之前，并让旧历史紧接本次连续用户输入，再接预激活 Skill 与初始 Run 状态。这两项调整只移动发送位置，增加不变内容形成连续前缀的机会。

本次输入可能包含多条连续 User 消息，不能只把最后一条抽到任务位置；它们之间的 anchored diff 随输入一起保留顺序。顶部 Skill/协作目录和预激活说明只来自 Run 初始组装；后来发生的 Skill 激活、恢复 full snapshot、状态 diff、同步 ToolResult 和异步 User 回应都留在当前 Run 的因果时间线，不能按内容类型全局提前。

Run 结束后，第 8–11 项自然成为下一 Run 的第 7 项：原有用户消息、正文和工具日志继续保留；附件的已提取文本与图片引用、预激活 Skill 完整说明、初始和后续 Run 状态则通过 `ContextMaterial` 在原位置重放，不再次读取当前 Skill 文件或重新提取旧附件。运行过程中激活的 Skill、恢复状态及观察以同一种记录追加。旧 Run 的状态是历史观察，当前有效权限仍只由当前能力快照、Run State 和 Host 校验决定。

```text
上一 Run：前缀 + 旧 7 + 本轮 8–11 + 即时尾部 12–14
下一 Run：前缀 + [旧 7 + 上轮 8–11（含最终回复）]
               + 新一轮 8–11 + 新的即时尾部 12–14

方括号部分 = 新一轮的 7
```

普通换轮、tools/前缀不变且没有压缩时，上一请求的输入与已发生事件保留相同 provider 消息格式；新回复和新输入追加在其后。当前 Todo、单次修复和未完成文件事务仍是请求尾部，不承诺跨 Run 保留该尾部的缓存。压缩、Provider 切换、能力 Schema/指南或初始目录变化仍会改变共同前缀。

`AssistantNarration` 通过 `provider_turn_id` 和 `first_tool_call_id` 关联所属响应及首个 canonical 工具调用。Generic 的首个 Tool Call 投影已经含有该响应正文时，历史组装不再追加它的独立 narration；原生 Provider 恢复私有 continuation 后也只撤下对应响应的独立正文。不能按文本去重，其他响应即使正文相同也必须保留。私有 MCP 参数和 FileChange successor 不为缓存写进公共历史。

`RunBootstrap`、`RunInput`、`RunTimeline` 和 `CapabilityInstructions` 是附加布局标签，保留原来源、角色、内容、图片、工具参数和绑定。物理重排不改作用域、保留策略、SQLite journal、Trace sequence、压缩游标或权限判定。摘要生成请求有独立的输入契约，不套用主 Agent 请求布局。

工具 Schema 不放入上述消息序列。请求的独立 `tools` 字段保持“稳定工具按名称排序，再拼接动态工具按名称排序”；稳定工具的使用指导继续位于稳定 system，动态能力专项指导只来自对应扩展。

每次自然请求边界先冻结能力快照，再由同一快照生成 Schema、能力指南和 World State。会话中关闭能力后，下一次请求撤下对应 Schema 与指南，通过原时间位置的 World State diff 表达不可用及原因；diff 只记录状态，不重新携带指南。已经发出的请求无法追溯撤回，迟到执行仍由 Host 重新检查实时策略；设置切换本身不额外调用模型。人机交互关闭也不撤销已接纳问题的回答与恢复。

此布局改善出现相同前缀的机会，不保证缓存命中或命中率提升。目录或能力指南变化时，其后长历史的共同前缀也可能失效；能力指南每次重建而不为缓存冻结。此次没有添加 Provider `cache_control`，厂商内部如何组合 tools/system/messages 仍由对应服务决定。

### 最终请求指纹与跨 Run 诊断

共同前缀必须按实际 Provider wire payload 比较，不能只检查消息是否包含某些文字。[Host 回归](../../crates/core-server/src/application/agent/tests/request_prefix.rs) 使用真实 Turn admission、SQLite、Runtime 和本地 Provider，比较跨 Run 的工具定义、消息、真实文件内容、成组 Tool 交换及 continuation。

普通发送由 Host 原子创建 user/assistant pair；Renderer 只乐观显示，不能迟到 upsert 覆盖首次持久化的创建时间、正文或 Run identity。上下文前缀使用这些稳定持久事实，既有错误历史不靠推测回写。[存储生命周期](./storage-and-data-lifecycle.md#消息创建与重试归属)维护新增/幂等重试与状态回存的共同边界。

回归覆盖首次请求后与终态后的迟到保存、保存后重启、终态状态回存、迟到 live checkpoint、启动响应丢失及失败 pair 补存，并核验上一轮最终正文/reasoning 在后续请求完整保留。本地序列化共同前缀一致不代表 Provider 一定命中缓存；不在当前维护文档保留历史一次性通过数量作为现状门禁。

### Todo 存储与提醒预算

Todo 的完整状态和模型提醒分别处理：权威状态、Renderer 事件及同 Run 检查点保存完整 ID、标题、备注和时间；500-token 启发式预算只限制 `request_only` 提醒正文，不再作为计划更新或检查点恢复的准入条件。消息结构开销仍由总上下文计量单独计入。最多 12 项以及 ID 64、标题 120、备注 240 字符的输入限制继续生效，恢复时也验证这些结构限制。

提醒使用 `revision`、完成计数和每项的 `ref`（当前列表从 1 开始的编号）/状态。标题和备注中的换行仅在提醒中折叠为空格，避免伪造额外编号行。超预算先省略备注，再优先缩短已完成项和靠后的待办标题，保留当前工作与阻碍；截短不切断 Unicode 字素，以 `…` 和省略声明标明，所有编号与状态始终保留。完成时不再追加重复的行为指导。

`todo_update` 仍是完整列表替换，支持两种条目：新条目或旧接口使用 `id? + title + status + note?`；引用条目使用 `ref + status + title? + note?`，根参数必须带最新 `expectedRevision`。引用从该 revision 的权威列表读取，省略的标题/备注原样保留，`note: ""` 可清空备注，真实 ID 不变；可以重排引用、混入新项或显式删除不再需要的项。`ref` 不能与 `id` 同时使用。过期 revision、重复/越界引用及非法字段原子拒绝，不推进状态、revision 或 ID 分配器。模型不能用省略后的标题重建原条目。

```json
{
  "expectedRevision": 3,
  "items": [
    { "ref": 1, "status": "completed" },
    { "ref": 2, "status": "in_progress" },
    { "title": "汇总验证结果", "status": "pending" }
  ]
}
```

这项预算修复保留 Todo 的请求尾部位置、Run 作用域及压缩排除规则，不改变跨 Run 历史格式。快照结构与扩展版本保持不变，无需重置开发聊天数据。

回归覆盖见 [Todo 预算与引用测试](../../crates/core/src/runtime/extensions/todo_budget_tests.rs) 和 [双 Provider Harness 测试](../../crates/core/src/runtime/tests/todo_budget.rs)：中文长计划、最大合法 Unicode 字段、过期/重复引用的原子拒绝、完整状态恢复，以及 OpenAI/Anthropic 兼容请求中的创建 → 受限提醒 → 引用完成 → 正常回复。2026-09-06 验证通过 Rust Core library 2,536 项测试（10 项按现有配置忽略）和 workspace all-targets Clippy。

### Conversation 与 Run World State

Conversation World State 在同一会话的多个 Run 之间延续。第一个真实模型请求建立 full，后续状态没有变化就不追加记录；有变化才在该请求的因果位置追加 diff。开启一个新 Run 本身不重新发送一份新的 Conversation full。

项目源文件夹由 `workspace.binding` 提供。Host 的 section 始终保留完整冻结绑定，包括真实路径、folder ID 与目录实体身份；模型只看 `alias/role/available/path`（`@workspace/<alias>`）及工作区总览。首次 full 和压缩 rebase 基线给出完整目录表；后续模型 diff 对该 section 使用 `op: patch`，可选 `set` 仅更新变化的顶层字段，`changes` 按 alias 表达 `added/removed/updated`。added/updated 的 `folder` 是该来源完整的安全投影，未列来源不变。`updated.reason=source_replaced` 说明同名目录来源被替换，模型应重新核实其文件事实；仅离线/恢复不视作换源，但恢复后仍须重新核实文件内容。模型投影按 alias 稳定排序，纯显示顺序变化不进入模型上下文。持久化日志仍使用原有完整 section add/replace/remove 和 revision 校验，patch 不参与文件系统授权。

工作区约定由 `workspace.instructions` 提供：Host 在每个采样边界读取各冻结文件夹根部的 `AGENTS.md`（同目录存在 `AGENTS.override.md` 时以它为准），按主要文件夹在先、其余按 alias 的稳定顺序聚合，合计上限 32 KiB，超出时在 section 内标注截断。Host 状态保留来源 alias、相对文件名、大小与内容哈希；模型投影只含 `@workspace/<alias>`、相对文件名、正文与截断标记，不含真实路径或身份。内容不变不追加记录；变化在请求因果位置追加 replace；全部约定消失则追加显式 remove。中途修改文件不需要重启对话或应用：下一次模型请求即携带新内容。

配置编辑的净变化在下一次新 HumanRoot 捕获工作区、首次采样时发布，不从前端保存事件直接注入。已启动的 Run、审批续接及子树 Wake 继续使用原冻结绑定。空闲预览用相同投影预演差异且不写日志；运行中圆环使用本轮实际请求状态。压缩只折叠已覆盖前缀，截止边界后的目录 patch 保持原因果位置；fork 和编辑重发同样沿用日志的精确截止边界。Runtime checkpoint v19 显式拒绝使用旧模型投影的检查点，不改写已有挂起任务的权限或历史文本。

被压缩覆盖的换源 patch 已折入 full，不再作为独立通知保留。摘要生成规则和摘要固定说明将文件内容、目录结构标为历史观察，提醒模型不能仅凭 alias 相同假定来源未变，依赖当前内容前重新读取；这同样适用于递归压缩后的摘要。

| 状态                                               | 所属与模型可见范围                                                                             |
| -------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| 权限、工作区、环境、交互偏好、模型选择             | Conversation，保留原有窄投影与 Host-only 私有字段                                              |
| 轻量/完整模式 `interaction.profile.contextProfile` | Conversation；记录当前 Run 冻结的 `full` / `minimal` 及简短说明，独立于工作模式和扩展开关      |
| 联网搜索可用性及原因 `web.search`                  | Conversation；由当前 Host 设置与配置就绪状态决定                                               |
| 人机交互可用性及原因 `human.interaction`           | Conversation；根身份、有人值守、设置和真实执行端口共同决定                                     |
| 浏览器用户开关 `builtin.capabilities.policy`       | Conversation；只描述用户是否允许申请该能力                                                     |
| 浏览器本任务激活状态 `builtin.capabilities`        | Run；不把设置开启误当成本任务已获授权                                                          |
| 激活的 Skills、附件库摘要                          | Run；各自保留现有作用域                                                                        |
| `tools.effective`                                  | 仅 Host；完整 toolset/revision 仍参与实际 Schema、执行校验和检查点，不再向模型重复列出工具名称 |

当前可用能力指南仍是 RequestOnly。Conversation 状态日志只记录状态和原因，不保存这些指南，也不重新挂载过去启用过的工具。

请求边界使用后端生成或核验的 `runId + assistantMessageId + requestIndex + afterTraceSequence`。最后一个字段指向该 assistant 内最近已闭合、模型可见的安全 Trace 项；首请求可为空。不能把这个位置退化为整个 assistant message，更不能按墙钟时间插入。因此同一回复中，关闭发生前的 narration、工具结果、之后的状态 diff 与后续回复能够保持顺序。

Host 在发出请求前用事务校验归属、运行状态、停止围栏、预期 head 和安全 Trace 前缀，写入状态与幂等 request receipt。即使没有 diff 也保留该请求的 receipt。状态初始标记为未确认观察，Provider 成功响应后才确认；发送失败不会把准备好的状态误标成已观察。未观察状态保留在上下文中、计为 `run_transient`，不可被压缩覆盖。观察确认失败会保留模型用量并结束当前执行，不重发已经完成的模型请求。

上下文帧和 checkpoint 同时保存 canonical 状态日志及其投影关联；不从模型可见文本反解 Host 状态。SQLite 增量缓存与冷重建使用同一日志。上下文预览在帧副本上计算当前设置对应的临时 full/diff 和指南，使用同一能力快照进行工具计量，不写日志、request receipt 或观察标记。

后端 `ContinuityIndexV2` 用于定位与审计，默认不作为主模型消息发送。附件、Skill Resource、Artifact 与 Browser download 必须先通过统一 locator 和各自授权解析，模型字符串本身不是文件系统权限。Host 可以从 `agent_nodes` 推导同一 Agent task tree 的 exact root scope，使父/子/兄弟复用 durable attachment/artifact/download；模型、Prompt 或 Renderer 不能自报 root identity，未知 scheme-like/`@namespace` locator 必须 fail closed。

Composer Folder Reference 是独立的输入来源：模型上下文有意包含所选目录 name/绝对路径，并提示先浏览再按需读取，不递归预载整个目录。Host 目录 identity 和可用性不进入模型正文；引用不替代 `workspace.binding`，不增加写权限或 `workspace.instructions` 发现根。纯附件、纯文件夹输入及运行中引导同样由 Host 构造材料，不为非空校验伪造用户正文；managed import ID 只是持久输入引用，真正材料构造仍复核附件归属及内容。详见[会话输入](../subsystems/conversation-inputs.md)。

Automation HumanRoot Turn 还会追加 `automation_execution` 来源的 retained、Run-scoped system item，包含
Host 从持久 Run 绑定构造的任务/Run identity、计划时间、上次运行时间和 trigger kind。它不修改用户
Prompt，也不参与可复用 Conversation configuration revision；Approval/Checkpoint 恢复保留同一上下文。
普通 Turn 不含该 item。完整调度边界见
[Scheduled Automation](../subsystems/scheduled-automations.md)。

组装器应保持确定性：相同持久日志、active head、权限/工具集版本和请求输入应产生相同的逻辑帧。增量缓存只是优化；删除、重写、回退、分叉或摘要变化后，全量重建必须得到相同结果。

### 统一历史材料的容量与压缩边界

- `ContextMaterial` 同时写入 Trace 和 model-context 投影，事件 ID 与序号不可变。source revision 包含材料种类、正文、图片身份和时间；提交压缩前的 CAS 能发现来源变化。
- 已结束 Run 的纯文字材料可以随完整前缀一起进入摘要；不能因为原来源叫 `SkillInstructions` 或 `WorldStateSnapshot` 就永久保护。
- 历史图片保存附件 ID、MIME 和内容摘要，Host 按会话/消息归属读取并核验 SHA-256 后恢复原图片。缺失或被篡改会拒绝发送，不静默去图，也不根据历史路径读任意文件。
- 文本摘要暂不替代图片。压缩覆盖包含图片的区间时，原图及同项文本作为保留材料继续占用上下文；容量报告计入这些图片。`CompactionRetained` 只是内部保护标记，防止同一已覆盖材料再次计入压缩来源或生成不前进的游标。
- 当前 Run 的材料在压缩后仍按原布局保留；该 Run 结束后，被摘要覆盖的纯文字不再恢复。fork/child snapshot 只继承边界内的观察材料，不继承激活权限、待投递工作或用量。
- 历史重建与活动帧合并时按 journal origin 对齐，不能同时计入两份材料；若恢复了活动 Run 的来源和布局标签，会使聚合计量缓存失效并重新计算分类，保证热缓存与冷重建的容量一致。

### 压缩与恢复时的布局

压缩后，模型消息、Tool 交换和 ContextMaterial 从权威 journal 重建；同一材料按 journal origin 对齐，当前帧与 baseline 不重复添加。尚需保留的 Run overlay 则继续存在。`request_order` 元数据将仍存活的 journal 项与 overlay 的相对位置关联起来，防止压缩后把后发的工具结果移到先发的 Skill/World State 之前；它不改变历史游标，也不重复复制已经重建的消息。该布局次序随私有 checkpoint 保存和恢复，恢复后的新 full Run World State 仍在恢复边界后追加。

共享 baseline 不保存其他 Run 的初始选择或当前输入标签；新 Run 在自己的帧副本上标记初始内容。Conversation-owned、尚未确认观察的状态可以随 baseline 保留，但计量和压缩仍识别其 `run_transient` 属性。模型发送与上下文预览使用相同布局投影。

压缩 World State rebase 使用精确 `ContextJournalCursor`：只折叠截止边界已被观察的前缀，后续请求 diff 原位保留，并在新 epoch 下续接。位于某个 Trace 项之后的请求状态不属于“覆盖到该 Trace 项”的前缀。分支只复制选定边界内的状态历史，并重映射 Run、assistant 和 Trace 引用；不复制 prepared 请求回执、授权快照或用量。删除和重写清理受影响状态尾缀及其请求回执，防止保留悬空的请求身份。

## 容量判断

`ContextCapacityDetector` 使用模型、API style、工具定义和 `ContextBudgetReport` 判断请求是否可发送。安全边界必须同时考虑：

- Provider/model context window；
- 当前请求估算输入；
- 为输出保留的预算；
- estimator 误差的 safety margin；
- Provider continuation 等非普通文本投影。

估算与实际 Provider usage 是两个不同事实：前者用于发送前防溢出，后者用于发送后计费和诊断。不能用返回 usage 回写历史内容，也不能因为一次估算偏差改变已经发送的请求。

自动压缩的触发判断、发送容量门禁和运行中圆环直接消费同一份完整请求计量报告，使用相同的输入总量、输出预留、安全余量和可用输入容量。压缩完成后重新组装并计量最终请求，不能从另一个历史计数或 Provider usage 推导阈值。空闲会话、回复完成及其缓存重建使用完整的下一次请求预览，包含当前工具 schema、Skill 目录、协作指南与目录（若开启）和 Run World State 等临时内容；已完成 Run 的 Skill 激活、Automation 执行标记与恢复状态不作为下一次请求的活动配置。正在运行的会话使用本轮冻结的协作策略，且不能在本轮已持久化的 Run World State 之外重复添加下一轮快照。持久 baseline 只是可复用历史，不能把它的裸计数作为完整请求大小发布。相同历史与配置的冷缓存、热缓存和终态预览必须一致；实际压缩或配置变化仍可以让占用下降。

轻量模式与完整模式通过同一请求准备入口选择基础提示词、工具定义和 World State，再生成完整请求计量；模式及最终定义参与配置指纹。设置保存后，空闲预览采用下一轮模式，正在运行及等待审批的 Run 保持冻结模式，不能把下一轮预估覆盖本轮快照。自动压缩阈值仍使用这份相同的计量结果，不另设“轻量模式 Token 算法”。

切换模式不删除或重写旧聊天、Trace、已观察材料和压缩摘要；旧工具结果及历史状态仍按原有日志规则计入上下文。新模式精简的是当前基础输入，不能据此承诺长聊天立即降至新聊天大小。历史中的工具或模式说明不会恢复当前已撤下的工具。

### 请求输出上限与上下文预留

普通对话不再由 Renderer 或 Runtime 统一注入 `maxTokens: 30000`。`runtime/output_budget.rs` 一次解析两个独立结果：可选的 HTTP 输出上限和非零的内部输出预留。发送前检查、运行中自动压缩、空闲预览和配置指纹使用同一解析入口；未知服务的预留估计不得反向写成请求限制。

| Profile                   | 默认请求字段                   | 内部输出预留                                      |
| ------------------------- | ------------------------------ | ------------------------------------------------- |
| 通用 OpenAI-compatible    | 不发送 `max_tokens`            | 30,000，未知默认值的估计                          |
| DeepSeek Flash / Pro      | 不发送 `max_tokens`            | 非思考 8,192；思考默认 65,536；max effort 131,072 |
| Moonshot K3               | 不发送 `max_completion_tokens` | 131,072                                           |
| Moonshot K2.6 / K2.7 Code | 不发送 `max_completion_tokens` | 30,000，未确认默认值的估计                        |
| 通用 Anthropic Messages   | 必填 `max_tokens: 30000`       | 30,000，保留现有兼容值                            |

以上已知默认值依据 [DeepSeek Chat Completions](https://api-docs.deepseek.com/api/create-chat-completion/) 和 [Kimi API 错误说明](https://www.kimi.com/help/kimi-api/api-troubleshooting) 核对；[Kimi API 概览](https://www.kimi.com/help/kimi-api/api-overview) 允许采用服务端默认输出参数。[Anthropic Messages](https://platform.claude.com/docs/en/api/messages/create) 仍要求显式输出上限。DeepSeek 的 disabled thinking 与非默认 effort 冲突组合继续由既有 Profile 校验拒绝。通用兼容 Profile 不按模型名猜测厂商；第三方转发服务的实际默认值仍可能不同。

省略参数不等于无限输出，也不保证所有服务的默认值都更大。旧任务/审批恢复快照中明确保存的正整数上限保持原值，包括大于原 128,000 通用截断值的预算；0 明确拒绝，不变成默认值。参数省略与显式 30,000 即使预留相同，也有不同配置指纹。工具续接始终使用本轮冻结的模板；审批、重启、自动化和子 Agent 不额外补预算参数。

输出预留和安全余量不能挤占全部输入窗口；不满足时继续显式报告容量不足，不清零预留或偷偷提高模型窗口。特别是 131,072 的预留无法放入旧的 128,000 默认上下文窗口，需在现有模型配置中核对实际支持的窗口。未配置窗口的 Rust Core 调用仍保留非零预留，但无法承诺容量门禁；Host 仍使用已有的窗口默认规则。本阶段不新增预算设置界面、模型预算数据库或自动重发策略。

模型编辑页点击保存时，Host 在模型配置和凭据落盘前复用同一输出预留与安全余量规则进行基础容量校验。无输入空间时返回有界的结构化 `invalid_context_capacity_configuration` 拒绝，Renderer 复用单按钮“知道了”弹窗，保留全部编辑内容，关闭后回到窗口输入框；该页面不再重复显示保存失败 toast。前端不按模型名推测预算、不复制容量公式，也不自动增大窗口。此校验只保证配置留有输入空间，不代表任意历史/工具结果都能装入，因此发送前完整请求门禁仍保留。

设置保存是全量列表请求。新增模型、窗口/Profile/模型/协议变化及重新启用均自动校验；模型编辑页另传请求级 `validateContextCapacityModelId`，使未修改但明确点击保存的现有模型也接受校验。未编辑的历史无效模型不阻塞其他模型或全局设置的保存；显式校验目标必须存在于本次请求，不作为持久设置保存。错误公开字段仅包含模型身份、显示名称及容量数字，不返回凭据或底层存储错误。

压缩摘要属于独立内部请求：始终发送显式有限预算，最大 30,000，再按较小的显式任务上限、原前缀可缩减空间及实际窗口余量收紧。普通回复省略上限或采用更大的服务端额度，不得扩张压缩摘要预算。模型触及自身输出上限、返回空结果或异常断流，继续走已有异常持久化与红色提示行。

## 稳定压缩游标

`ContextJournalCursor` 只能落在完整逻辑项之后：

- 完整 user/assistant message；
- public narration；
- 已闭合 Tool Result。

游标不得位于 Tool Call 与 Tool Result 之间，也不能覆盖 `run_transient`。规划器选择的是可被单个替换块替代的旧前缀，而不是简单删除最老若干 token。

Host 重建上下文缓存时，模型 baseline 使用摘要与未覆盖的 Trace/模型日志片段；`committed_activity_items` 则始终用完整 Trace 与完整模型日志配对计数。不能把完整 Trace 与压缩后的局部日志一起校验，也不能把局部片段长度当作完整 journal 的增量游标，否则会误报历史缺失或在后续追加时重新引入已压缩内容。终态统计刷新、缓存失效恢复和压缩后的运行中重建遵守同一规则。

## 摘要与 Continuity Index

`ContextCompactionSummary` 是不可变派生记录，包含上一摘要、稳定游标、源 revision、语义摘要、确定性 Continuity Index、计量诊断、模型和时间。active head 单独保存，因此提交失败不会破坏旧摘要。

摘要模型只读取：

```text
previousSummary + 新选中的原始日志前缀
```

摘要生成提示词以工作交接为目标：为继续同一任务的下一位助手留下简洁备忘录，交代用户目标、已验证进展、关键决定、未完成事项和下一步，帮助其避免重复劳动。请求仍采用专用 system 与包含历史 JSON 的 user 消息，不追加真实用户聊天、不携带工具；自动、手动及 Provider 转换压缩继续共用生成器。长度目标是软目标，不能为达标丢失必要事实；提示词风格调整不改变覆盖边界、计量、校验和原子提交，也不代表复用了普通对话的前缀缓存。

输入历史按不可信数据处理。用户要求、助手主张、Tool/审批/终态证据必须区分；没有成功 Tool 证据的自述不得摘要成“已完成”。模型只生成 Markdown 语义正文，游标、revision、Continuity、token 统计和 active head 全由后端计算。

语义摘要应使用稳定的小节：

- Objective and constraints
- Confirmed state and decisions
- Completed work and artifacts
- Failures, approvals, and cautions
- Open work and next action

输出为空、被截断、夹带工具调用或未来替换块没有严格小于原前缀时，不得提交。

`ContinuityIndexV2` 只保存固定配额的 Message/Trace/Archive 引用和分类计数，不复制正文。当前硬上限为 1,500 tokens，常规目标不超过 800 tokens；它不进入主模型的日常 token 账单。引用必须属于同一 conversation 且仍存在。

## 压缩状态机

```text
capacity exceeded
  -> plan safe prefix
  -> receipt: planned
  -> prepare authoritative prefix + sourceRevision
  -> receipt: generating
  -> generate summary outside SQLite write transaction
  -> receipt: committing
  -> transaction re-reads prefix and checks head/revision/cursor
      -> commit observation + immutable summary + lineage + head + receipt
      -> or reject stale draft
  -> rebuild ContextFrame from SQLite
  -> remeasure and either send or compact again
```

摘要生成期间允许原始日志继续变化，但 commit 必须在事务内重新验证。游标之后追加不使摘要失效；覆盖范围内的编辑、删除或回退会改变 `sourceRevision`，使派生摘要失效或回到有效祖先。每个主请求有有限压缩尝试，无法获得实际缩小时返回明确容量错误。

## ModelRequestObservation

每次实际 Provider 请求在发送边界创建版本化观测：purpose、各 ContextFrame 分类估算、estimator identity/version、context revision、Provider usage、finish reason/有界错误及 billable request count。

这里的 ModelRequestObservation 是请求计量/诊断记录，不是 `read_file` 产生的 File Observation。它不保存 prompt、消息正文、Tool Result、网页/文件正文或 API key。普通请求的观测持久化失败不能重放已经完成的模型请求；压缩成功时 observation 与 summary/head/receipt 同事务提交。

## 分叉、删除与回退对上下文的影响

- 普通 UI 分叉和子 Agent snapshot 都复制已选择的完整终态 Turn，而不是共享原 conversation 的活动视图。
- 消息、Trace 和文件变更证据使用同一份已选消息集合；编辑重发所替代的旧 Turn 即使保留审计索引，也不进入新分支。有效消息的证据归属仍须严格校验，不能用忽略映射错误代替边界筛选。
- `fork_turns=all` 可包含可见摘要链；最近 N 个 Turn 只复制选中的完整终态 Turn，不继承更老摘要；`none` 不复制历史。
- 运行中尾部、Usage、普通请求 Observation、Command Session 运行态、Checkpoint、draft 和可变 World State 不进入历史快照。
- 删除/回退使依赖被删除前缀的摘要失效；分叉会重写 Message/Trace/Archive 引用。

具体复制边界和 Exact Archive 规则见[Conversation Trace 与 Exact Archive](./conversation-trace-and-archive.md)。

## 不变量

1. `ContextAssembler` 是模型历史的唯一结构化入口。
2. ContextFrame 计量同时服务容量判断、压缩和 UI，不维护第二套 token 口径。
3. 压缩只替换模型视图，不删除原始消息、Trace 或 Exact Archive。
4. Tool Call/Tool Result 闭环不可被压缩边界拆开。
5. 模型无权选择 coveredThrough、sourceRevision、Continuity ref 或 active head。
6. `run_transient` 在主模型成功观察前不可压缩。
7. Provider 私有 continuation 不得伪装成公开上下文文本。
8. Automation execution context 只能由 Host 从持久 Run 构造，Renderer 或 Prompt 不能声明该身份。
9. pending assistant placeholder 与 collaboration timeline 都是展示状态，不得进入 Provider-neutral 历史。
10. FileChange call body/Observation authority 不进入普通 durable Trace；successor 只能由 commit 后验证生成并保留在模型/私有 checkpoint 边界。
11. Agent-tree 资源共享 scope 必须由 Host 从持久节点解析，locator 文本不构成授权。
12. 模型请求布局与 journal chronology 分开；后发 Skill、恢复状态和完整 Tool 闭环不得移到其因果前驱之前。
13. 当前能力 Schema、RequestOnly 指南及 World State 由同一请求快照产生，关闭事实不恢复已撤下的指南或执行权限。

## 代码真源

- 组装与帧：`crates/core/src/context/`
- 请求布局：`crates/core/src/context/frame/request_layout.rs`、`frame/measurement_state.rs`、`frame/checkpoint.rs`
- Runtime 容量入口：`crates/core/src/runtime.rs`
- Conversation 状态请求边界：`crates/core/src/runtime/conversation_world_state.rs`、`crates/core-server/src/application/agent/conversation_world_state.rs`
- Conversation 状态事务与请求回执：`crates/core/src/storage/world_state_repository/`
- 容量与 UI snapshot：`crates/core-server/src/application/agent/context_window.rs`
- 压缩：`crates/core/src/runtime/context_compaction.rs`、`crates/core/src/runtime/context_compaction_model.rs`、`crates/core-server/src/application/agent/context_compaction.rs`
- 摘要存储：`crates/core/src/storage/context_compaction_repository.rs`
- Observation：`crates/core/src/model_request_observation.rs`、`storage/model_request_observation_repository.rs`
- 逻辑 Trace：`crates/core/src/conversation_trace.rs`
- Automation system context：`crates/core/src/protocol.rs`、`crates/core/src/runtime/preparation.rs`
- FileChange context projection：`crates/core/src/tools/apply_patch.rs`、`crates/core/src/file_change_support.rs`、`crates/core/src/runtime/checkpoint.rs`
- Tree resource scope：`crates/core/src/storage/agent_tree_resource_scope.rs`、`crates/core/src/resource_locator.rs`

## 测试

- `crates/core/src/context/frame/tests.rs`
- `crates/core/src/runtime/tests/conversation_context.rs`
- `crates/core/src/runtime/tests/compaction_and_tool_flow.rs`
- `crates/core/src/runtime/tests/request_layout.rs`
- `crates/core/src/runtime/tests/conversation_world_state.rs`
- `crates/core/src/storage/world_state_repository.rs` 内单元测试
- `crates/core/src/storage/context_compaction_repository/tests.rs`
- `crates/core-server/src/application/agent/tests/context_runtime.rs`
- `crates/core-server/src/application/agent/tests/context_history.rs`
- `crates/core-server/src/application/agent/tests/conversation_world_state.rs`
- `crates/core-server/src/application/agent/tests/world_state_compaction_boundary.rs`
- `crates/core-server/src/application/agent/context_compaction.rs` 内单元测试

### 维护回归与版本边界

请求布局、跨 Run 材料、压缩恢复与 Provider continuation 应联合验证，不把某个历史轮次的测试数量当作当前完成门禁。重点补充入口：

- [输出预算回归](../../crates/core/src/runtime/tests/output_budget.rs)：省略 wire 上限、内部预留与恢复指纹。
- [统一历史回归](../../crates/core/src/runtime/tests/unified_history.rs)：真实 Runtime、SQLite 重启、原图引用、冷/热容量与跨 Run 前缀。
- [请求前缀回归](../../crates/core-server/src/application/agent/tests/request_prefix.rs)：Host admission、迟到 Renderer upsert、终态及持久化回执。
- [工作区指令回归](../../crates/core-server/src/application/agent/tests/workspace_instructions.rs)：同一次发现用于预览与请求，replace/remove 及目录身份复验。
- [分支历史回归](../../crates/core/src/storage/conversation_fork_repository/tests.rs)：精确边界、上下文来源、图片和身份重映射。

当前 SQLite 版本与升级/reset 边界统一以[存储生命周期](./storage-and-data-lifecycle.md#schema-发布策略)为准。Trace 为 v6（[常量](../../crates/core/src/conversation_trace/model.rs)），Runtime checkpoint 为 v19（[常量](../../crates/core/src/protocol/checkpoint.rs)），私有恢复信封为 v14（[校验](../../crates/core-server/src/application/agent/persisted_resume_input.rs)）。这三种版本独立于数据库版本；拒绝旧 checkpoint 不意味着数据库必须删除历史，也不意味着 reset 能恢复任意旧备份配置。

## 普通后端历史事件

忽略非阻塞交互不再占用请求尾部的独立层。每次结算产生一个普通 `BackendState` Trace，
模型正文为 `{type: human_interaction_status, requestId, status: ignored}`，经过现有
`backend_observed_state` 包装进入因果历史。它没有审批、用户回应或推理唤醒语义。

运行中的事件在下一次完整工具交换后的自然采样边界进入；空闲后的事件以通用
`after_message` 位置排在最终回复之后。journal 的该位置也位于 Message cursor 之后，
因此后来发生的忽略不会更改已经压缩的消息前缀。历史裁剪使用 Host 的
`conversationCompletionCovered` 标记说明最终回复已经被摘要覆盖，防止后置事件保留时重复终态。

压缩使用普通历史规则，不特意保留或重新注入忽略状态；活动 Run 的事件与权威 journal
重建结果通过 trace origin 去重。分支继承边界内的冻结事实，不复制可执行投递；仍 open 的非阻塞问题由专用分支事务创建目标身份，并且只在可认证回执中重写请求 ID。
完整实现见[人机交互](../subsystems/human-interaction.md#非阻塞路径先接纳后投递)。

## 文件事务上下文

文件事务的上下文现已按寿命拆分：固定规则在稳定前缀，已完成结果归普通工具历史，尾部只含
当前未完成事务的状态与精确游标。纯文字违规提醒只进入下一次请求；工具旁白被屏蔽的事实以普通
`BackendState` 留在其完整工具批次之后，随历史压缩，不长期保护过期操作指令。
详见 [FileChange 模型上下文](../subsystems/file-change.md#模型上下文的三种寿命)。

## 变更检查表

- [ ] 新上下文来源被明确分类为 fixed、durable、run-transient 或 request-only。
- [ ] 容量检测、实际 request builder、Observation 和 UI 使用同一计量结果。
- [ ] 新日志项定义安全压缩边界，并覆盖 Tool Call/Tool Result 原子性。
- [ ] FileChange call body、Observation 与 successor 在 Trace/model/private checkpoint 之间保持脱权投影。
- [ ] 新虚拟资源由统一 locator 分类，并在组装前用 Host-resolved Conversation/Project/tree scope 授权。
- [ ] 摘要 schema、source revision、lineage、fork/rewrite/delete 行为均有迁移或失败策略。
- [ ] Provider 新增的私有内容由 Provider policy 计量，不泄漏进普通消息。
- [ ] 压缩失败、过期提交、取消和重启测试不改变旧 active head。
- [ ] 更新 Trace/Archive 文档和上下文契约测试。

## 当前限制

- token estimator 是发送前估算，不能保证与所有兼容 Provider 的计费完全一致。
- 压缩依赖当前模型生成有效且更小的摘要；最多有限尝试，不承诺任何输入都可压缩。
- ContextFrame 的增量缓存只在 revision 连续时有效，复杂重写会退回全量重建。
- Continuity Index 是有限引用集合，不是完整目录；精确内容必须通过历史工具打开。
- 正在执行且未提交的流式模型片段不会成为可恢复的长期上下文。
- Run 结算后协作活动在 owner 会话的消息间独立展示，不回写冻结回复，也不由这些 UI 活动反向组装模型上下文。
