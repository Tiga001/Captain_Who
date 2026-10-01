---
status: current
audience: developers
owner: engineering
last_verified: 2026-10-01
---

# Tool Result 上限、分页与恢复

本文定义 Tool Result 从来源捕获到模型投影的统一大小契约。具体数值的代码真源是各模块常量与测试；本文记录当前公共边界和修改原则。各投影字段见[Tool Result 消费者矩阵](./tool-result-consumer-matrix.md)，权限边界见[Tool 体系、权限与审批](./tools-permissions-and-approvals.md)，FileChange 的完整状态与 authority 见[FileChange 子系统](./file-change.md)，后台 Run 的业务语义见[Scheduled Automation 子系统](./scheduled-automations.md)。

## 职责边界

本子系统负责来源安全捕获、语义分页、消费者限长、统一模型 gate 和恢复标记。它不定义产品计费配额、不决定 Tool 权限，也不把被来源丢弃的内容伪装为可从 Archive 恢复。

## 唯一主链路

```text
Tool / Provider / OS process
  -> source-specific safety capture or semantic pagination
  -> Canonical Tool Result
      +-> Exact Archive：安全捕获原文
      +-> Event / Trace / Checkpoint：独立有界投影
      `-> Model Projection
            -> shared result gate (ordinary tools: 10K; collaboration/workflow reads: intact tool-owned results)
            -> model context and usage accounting
```

静态 Tool、Runtime Extension、审批后由 Core Server 执行的 Tool、外部 MCP Server 和内置 Capability 的文本模型结果使用同一个 Gate。普通工具采用 `MODEL_TOOL_RESULT_MAX_TOKENS = 10_000`。六个 Agent 协作工具承载完整通信消息，按可信工具身份保留全文；工作流只读工具 `workflow_get_mailbox` 与 `workflow_get_state` 同样保留工具自行构造的完整页和图引用，避免通用裁剪丢失消息却保留越过它们的 cursor，或删除拓扑节点/连线后留下悬空引用。此处按精确工具名称单独登记两个工作流工具，不改变六个协作工具的注册或权限范围。实时结果、预提交回执和历史恢复遵守相同政策；这些结果仍计入整轮上下文测量、压缩和发送容量检查。最终发送前的豁免必须匹配实际 Tool Call 的 ID 与名称，不能根据正文中的字段推断。

完整通信正文指显式发送的任务、消息和成果。子 Agent 的最终回复只保留在自身会话；自动 `Result` 是 Host 生成的结束通知，包含身份、终态、必要错误和产物引用，不复制或截取最终回复。详细成果由子 Agent 使用 `send_message` 汇报给直接父级，沿用上述全文交付规则。

职责上，用户提问答案与协作消息属于通信交付，完整性应由各自消息协议和整轮上下文预算处理，不应转交 `conversation_history` 补读。本轮没有为用户答案新增单结果 Gate 豁免，也没有添加通用结果分页协议；答案的现有交付与容量行为保持不变。

## 四种限制语义

| 类型                        | 含义                                            | 恢复方式                                                            |
| --------------------------- | ----------------------------------------------- | ------------------------------------------------------------------- |
| Source truncation           | Tool/Provider 在 canonical/Archive 前已丢失内容 | 无法恢复遗漏部分；必须报告原因与已知数量                            |
| Semantic pagination         | 完整集合由稳定游标分页                          | `nextCursor`/`continueWith`，继续时重新授权                         |
| Consumer projection limit   | E/T/C 只需要有界视图                            | 不影响 canonical 或 A                                               |
| Model projection truncation | M 投影超过共享 10K                              | 仅原工具已有的范围、cursor、`continueWith` 或产物读取；可能无法恢复 |

`truncated=true` 可能只是当前页未完，不能单独解释为来源丢失。权威来源字段是 `truncatedAtSource`、捕获/遗漏量、`stopReason` 和恢复标记。

`archivedCompletely=true` 只证明后端完整归档“实际收到并通过安全清洗的内容”。Provider 未声明完整性时使用 `sourceCompleteness=unknown`；不得把“后端未观察到截断”伪造成“上游完整”。

## 当前公共边界

| Tool/类别                    | 来源或安全硬限                                                                                               | 模型交付                                 | 溢出恢复                                                 |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------ | ---------------------------------------- | -------------------------------------------------------- |
| 共享 Exact Text Capture      | 单次 64 MiB                                                                                                  | 独立于 M                                 | 内部 Archive 分块；不是普通工具的公共续读入口            |
| `search_code`                | walk 20,000；单文件 512 KiB；单匹配行有界                                                                    | 预算感知页 + 10K                         | opaque cursor；超长行用 `read_file`                      |
| `search_files`               | walk 20,000；每页最多 200                                                                                    | 预算感知页 + 10K                         | opaque cursor                                            |
| `attachments_list*`          | 请求页最多 500                                                                                               | 预算感知页 + 10K                         | opaque cursor                                            |
| `workspace_map`              | depth 8、tree 1,000、walk 20,000                                                                             | summary/treeText/coverage                | 缩小 `focusPath` 重查                                    |
| 文档文本读取                 | 文件 25 MiB；OOXML 单 entry 16 MiB、总 XML 64 MiB                                                            | canonical text + 10K                     | 仅已有读取范围；无相应能力时明确无法补取                 |
| 文件输入 mount               | 最多 16 项；普通文件流式校验/复制，无统一单项 64 MiB 或合计 128 MiB 旧上限；视觉输入单项 8 MiB               | 不直接等于 Tool Result                   | 来源/解析器仍有独立边界，按结构化错误处理                |
| `web_search`                 | 请求/模型最多 8 条；Provider 另有限制                                                                        | 搜索产品页 + 10K                         | 用 `web_fetch` 读取来源；不承诺搜索结果有可调用的 cursor |
| `web_fetch`                  | Provider/传输安全限；Exact Capture 64 MiB                                                                    | M 统一 10K；E/C 可有兼容字符限           | 当前无通用正文分页；明确标记截断                         |
| `apply_patch` Direct         | complete content 32 KiB；1–128 edits；edit 后目标 240,000 UTF-8 bytes                                        | receipt/successor + 10K                  | 超 Direct 边界改用 Staged；冲突后重新 `read_file`        |
| `apply_patch` Staged         | append chunk 1 MiB；每次 edit 1–128 项；transaction 4 MiB；draft TTL 7 天                                    | progress/receipt + 10K                   | `status`/继续 mutation/`commit`/`abort`                  |
| FileChange Observation/Diff  | Observation claim 10 分钟、每 Run 1,024；内容/Diff/历史页默认 50,000 chars，夹在 1,000–100,000               | successor 仅 M/私有 C                    | `nextOffset`；无 successor 时重新 `read_file`            |
| Command/Office/Skill Script  | stdout/stderr 共享 64 MiB；每 stream 128 KiB 预览                                                            | 预览 + 10K                               | 已有 session 增量或可读产物；不重跑操作来补输出          |
| `run_command`                | command 16,000 字符；timeout 最多 600 秒                                                                     | terminal 或 running session receipt      | `command_session` 观察原会话；无通用终态日志分页         |
| `command_session`            | 持久 transcript 256 KiB/最多 2,048 chunks；单次模型增量最多 4 KiB                                            | 安全增量 + 10K                           | 后续 wait；不递归 Archive                                |
| `read_file` / Skill Resource | 文件/资源自己的读取限                                                                                        | 在预算内构造稳定页，再经防御性 gate      | byte/line cursor 或 `continueWith`                       |
| Skill Script                 | 128 个 argv、总 64 KiB；requirements 最多 128 项；默认 120 秒、最多 600 秒                                   | preflight/执行结果 + 10K                 | 已有可读产物；内部 Archive 不提供普通结果续读            |
| Office                       | 默认 120 秒、最多 600 秒；文档最多 512 MiB；页/尺寸另有格式限                                                | 语义 outputs + 进程预览                  | 已有产物或只读范围；进程日志不承诺可补取                 |
| 图像生成 Artifact            | 默认下载最大 32 MiB；尺寸/像素和 HTTP 均有限制                                                               | 只传 Artifact identity/URI               | 无文本分页；重新生成不是恢复                             |
| Browser Artifact projection  | 最多 16 个临时引用；单个 128 MiB；最长 24 小时引用期                                                         | 安全 ref/元数据；截图可有 image readPath | Renderer 经 Main broker 预览/导出；截图可用 `read_image` |
| Browser Download reference   | schema v2；每次最多 16 个无路径引用；单项最大 2 GiB；Rust Core 持久记录                                      | `browser-download:<uuid>` + 安全元数据   | 后续文件输入重新校验 Conversation/tree/Project scope     |
| 外部 MCP Server              | raw result 4 MiB；128 blocks；文本 16 KiB、结构 JSON 8 KiB；结构深度 32/节点 4,096；encoded media 合计 2 MiB | 安全投影 + 10K                           | 仅服务原生读取/分页/产物契约；不臆造参数或重放副作用     |
| 内置 Capability Tool         | 参数 64 KiB、raw result 4 MiB、model projection 64 KiB，并有 JSON 深度/节点限                                | Core Server/Main 安全投影 + 10K          | 按 Tool/Artifact 契约；unknown 禁止自动重放              |
| `conversation_history`       | 仅当前有效摘要覆盖的安全持久模型上下文；按字符分页                                                           | 预算感知历史页 + 10K                     | 边界内 `open`/navigation；拒绝原始 Archive 路由          |
| `automation_report`          | summary 2 KiB；Run result preview 8 KiB；error message 4 KiB                                                 | 小型确认 receipt + 10K                   | 无分页；首次报告持久化，终态 fallback 按 UTF-8 安全裁剪  |
| 协作 Tool                    | 保留 Mailbox 总积压背压与单批条数限制；正文无固定业务字数上限                                                | 完整消息；使用整轮上下文预算             | 超整轮容量明确失败并保留原文；不进 Exact Archive         |
| `workflow_get_mailbox`       | 默认 20/最多 50 条；单页正文最多 256,000 bytes；单消息沿用发送时 128,000 bytes 上限                            | 完整工具页；使用整轮上下文预算           | `nextCursor`；正文预算省略记录可按 `messageId` 查询       |
| `workflow_get_state`         | 定义最多 128 节点/512 连线；全局职责文本预览 512 chars，简介/背景预览 4,096 chars                              | 完整图引用与工具预览；整轮上下文预算     | `nodeId` 聚焦取得完整节点和公共背景；明确 `truncatedFields` |

表中数字是当前核验快照。修改任何一项时必须以常量和测试为准，并同步本文；不要从文档生成安全配置。

Composer 原始附件的导入、模型视觉输入与 Tool Result 是不同阶段：managed import 以最多 512 KiB 的分块持久化原件，不再用旧的普通附件 8 MiB 上限；Runtime 图片上下文仍有单图/合计预算，普通文档读取也仍有解析限。移除输入 mount 的统一大小限不会放宽 64 MiB Exact Capture、10K 普通工具结果 Gate 或 Browser 下载来源限制。完整输入链路见[会话输入](conversation-inputs.md)。

## Model Result Gate

Gate 使用当前模型/API style 的 `ContextTextBudget` 估算完整 Tool message，而不是简单按字符截断。处理顺序：

协作消息及两个工作流只读工具的语义结果按工具提供的内容交付，完整计量。工作流工具先执行自身的页数、正文预算和可恢复字段预览，中央 Gate 不再次删减集合；超整轮容量仍按正常上下文策略处理，不允许静默丢弃正文或图引用。其余工具采用下列规则：

1. 如果结果在 10K 内且无需补充截断标记，原样交付模型投影。
2. 若 Tool 提供语义分页，优先让 Tool 在预算内重新构造一页，保证 cursor 从模型实际看到的最后一项继续。
3. 只保留 Tool 自己提供的有效续读信息，不因建立 Exact Archive 而生成 `historyOpen` 或引导调用 `conversation_history`。
4. 仅做文本裁剪时保留结构化错误、完整性、已知遗漏量及原工具的读取指引；没有入口就明确标记截断，不虚构可恢复性。
5. 输出始终是有效 JSON；极小预算下返回有界的截断诊断，不能把无标记的半截 JSON 交给模型。

中央 Gate 只改变 M，不得回写 canonical/E/T/A/C，也不得把 projection truncation 标成 source truncation。

## FileChange 分页与续约

FileChange 的“正文组装上限”“模型结果上限”和“审查 Diff 页”是三类不同边界：

- Direct 只接受短完整 content；超过 32 KiB 应转换成 Staged，而不是截断后提交。structured edits 的结果若超过 240,000 bytes 同样拒绝。
- Staged append/edit 使用 `transactionId + index + expectedDraftRevision` 做乐观并发；4 MiB 是完整 transaction hard limit，不能用多次 append 绕过。
- `agent.readFileChange`、`agent.getFileChangeDiff` 与 `agent.getFileChangeHistoryDiff` 按 Unicode chars 分页，默认 50,000、最小 1,000、最大 100,000，并返回 `nextOffset`。它们是 Renderer/Host review API，不经过模型 10K gate。
- 成功写后的 Observation 没有分页语义：复核成功才把第一个/同一个 opaque ID 返回模型；缺失或 `observationRefreshRequired=true` 时只能重新 `read_file`，不能从历史 Diff 构造 authority。

## Scheduled Automation 报告与终态预览

Automation 有三层不同的文本上限，不能合并成一个“Tool Result 限制”：

- `automation_report.summary` 最多 **2048 UTF-8 字节**，必须 trim 后非空，只允许换行而拒绝其他控制字符；这是模型主动提交、每 Run 最多首次成功一次的用户可见报告。
- `automation_runs.result_preview` 最多 **8192 UTF-8 字节**。成功调用 `automation_report` 时实际内容不会超过 2048；未报告时，Scheduler 可从 terminal assistant message 生成最多 8192 字节的安全 fallback preview。
- `automation_runs.error_message` 最多 **4096 UTF-8 字节**；错误 code 最多 128 字节。Agent start/Trace 失败内容会移除不安全控制字符并按 Unicode 边界裁剪。

这些上限发生在 Automation 的持久业务投影中；随后返回给模型的 `automation_report` receipt 仍经过共享 10K token gate。8 KiB/4 KiB 裁剪没有 cursor 或 Exact Archive 恢复承诺，不能标成“完整归档后可继续读取”；完整 Conversation message/Trace 是否仍可访问由 Conversation 生命周期决定。

## Opaque cursor 契约

Cursor 绑定 Tool 类型、规范化 query/filter、授权 scope、conversation/project、必要的 workspace/目录 revision 和下一页位置。继续调用时重新执行权限与路径校验。

- cursor 被修改、跨会话使用、query 改变或快照 revision 变化时返回结构化失效错误；
- `total` 是稳定快照总数，`returned` 是本页数，`omitted` 是当前响应未包含数；
- `nextCursor` 必须从模型实际收到的最后一项之后开始；
- cursor 是定位信息，不是 bearer authorization，也不允许模型解析构造。

搜索通过重新扫描和指纹校验维持一致性，不长期持有文件系统 snapshot；文件变化可使 cursor 失效。

## Exact Archive 与进程输出

安全 UTF-8 文本按 SHA-256 标识、zstd 分块写入 Archive。命令、Office、Git 和 Skill Script 使用临时 spool 捕获 stdout/stderr；预览被截断不等于 archive 被截断。

Archive 保留内部审计证据，不为普通截断结果生成公共续读引用；压缩历史工具读取安全持久模型上下文，不开放原始 Archive。外部 MCP 和内置 Capability 并不统一存档所有正文；服务端或安全投影已省略的内容不能靠 Archive 恢复。只按当前 Schema 中实际存在的范围、分页、session 或产物读取能力取回内容，不给未知工具添加 cursor 参数，也不重复副作用操作。

`command_session` 和 `conversation_history` 不再次归档读回正文。图片、data URL、二进制 MCP Server blocks 和 Office 文件本体不进入文本 Archive，而由受管 Artifact/文件输入体系保存身份和授权。

## 不变量

1. 所有 M Tool Result 使用共享 Gate；普通工具限制 10K，可信协作回执及指定工作流只读结果保持工具语义完整并受整轮上下文容量约束。
2. 来源安全限先于 Archive；Archive 不能恢复上游未返回内容。
3. E/T/C 限长不改变 A，M 限长不改变其他消费者。
4. 可分页集合不得被通用字符裁剪后跳过条目。
5. 不确定副作用不是“输出过长”的恢复场景，禁止以 retry 解决。
6. 任何新 hard limit 都必须产生可诊断、可测试的 stop reason/omission metadata。
7. `conversation_history` 只读取有效摘要覆盖的原始历史；普通工具截断不触发历史工具路由。

## 代码真源

- 10K Gate：`crates/core/src/context/model_tool_result_gate.rs`
- Exact Capture：`crates/core/src/exact_capture.rs`
- 常用文件/搜索限：`crates/core/src/tools/limits.rs` 及各工具模块常量
- MCP 限制：`crates/core/src/tools/mcp/contracts.rs`、`crates/mcp-client/src/limits.rs`
- 命令 capture：`crates/core/src/command/output_capture.rs`、`command/types.rs`
- Artifact 限制：`crates/core/src/browser_artifacts.rs`、`image_generation/artifact.rs`
- Browser 下载：`crates/core/src/browser_downloads.rs`、`storage/service/browser_downloads.rs`
- FileChange：`crates/core/src/tools/apply_patch.rs`、`tools/file_change_staged.rs`、`file_change/observation.rs`、`crates/core-server/src/application/agent/approval.rs`
- Automation 报告/预览：`crates/core/src/tools/automation_report.rs`、`crates/core-server/src/application/agent/automation_turn.rs`、`crates/core/src/storage/automation_repository.rs`
- 契约 fixture：`crates/core/tests/fixtures/tool_result_projection_contract_v1.json`

## 测试

重点测试位于：

- `crates/core/src/context/model_tool_result_gate.rs`
- `crates/core/src/conversation_trace/tests.rs`
- `crates/core/src/tools/mcp/tests/`
- `crates/core/src/tools/run_command/tests/`
- `crates/core-server/src/application/agent/tests/context_history.rs`
- `crates/core-server/src/application/agent/tests/file_change_permissions.rs`
- `crates/core-server/src/application/agent/tests/staged_file_change_execution.rs`
- `crates/core/src/tools/automation_report.rs`
- `crates/core/src/storage/automation_repository/tests.rs`
- `crates/core-server/src/application/agent/tests/automation_turn.rs`
- `crates/core/tests/fixtures/tool_result_projection_contract_v1.json`

分页工具还应在各自模块覆盖 Unicode 边界、预算最小页、cursor 篡改、revision 变化和无恢复入口失败。

## 变更检查表

- [ ] 先判定新上限属于 source、pagination、consumer 还是 model。
- [ ] 给出计数、停止原因、完整性和安全恢复入口。
- [ ] Archive 在普通预览裁剪前接收安全捕获内容。
- [ ] 分页 cursor 绑定 query/scope/revision，覆盖篡改和跨会话测试。
- [ ] FileChange 限额分别覆盖 Direct、Staged、Observation 与历史 Diff；禁止用截断或 opaque ID 代替重新授权。
- [ ] 10K Gate 覆盖成功、错误、Unicode、结构 JSON 和无恢复入口的诚实截断；协作全文例外覆盖预提交、恢复与真实调用身份校验。
- [ ] 进程/网络取消不会因重试造成重复副作用。
- [ ] Automation 报告覆盖 2048/8192/4096 的 ASCII、Unicode 和控制字符边界，并验证首次写入不可覆盖。
- [ ] 同步消费者矩阵、常量测试和 Renderer 兼容字段。

## 当前限制

- token gate 使用 estimator，具体字符容量随模型/API style 变化。
- 64 MiB capture 是单次硬限，不适合无限日志；长任务应使用 session/cursor 分段消费。
- 某些外部 Provider 只提供 unknown completeness，系统不能证明其结果完整。
- 文档/网页解析在进入 capture 前可能有格式库或 Provider 自身安全限。
- 动态 MCP Server/Capability 可进一步收紧限制，但不得超过已审计的 Core Server/Main 最大值。
- 本轮没有新增文档、网页、进程日志或动态 MCP 的通用续读 API；缺少原生能力的遗漏可能无法补取。
- Automation 的终态 preview/error 裁剪当前没有独立分页或 Archive recovery ref；需要完整内容时应从仍存在的 Conversation/Trace 读取，而不是重放后台 Run。
