---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-23
---

# Tool Result 消费者矩阵

本文记录 canonical Tool Call/Tool Result 到各消费者的职责映射。它是评审指南，不是运行时 allowlist；机器可校验的字段契约以 `ToolRegistry` 投影函数和 `crates/core/tests/fixtures/tool_result_projection_contract_v1.json` 为准。权限与审批见[Tool 体系、权限与审批](./tools-permissions-and-approvals.md)，大小边界见[Tool Result 上限、分页与恢复](./tool-result-limits.md)，后台报告的业务消费者见[Scheduled Automation 子系统](./scheduled-automations.md)。

## 职责边界

本文负责说明字段应该到达哪些消费者，以及哪些内容必须隔离。它不定义 Tool 输入 schema、不决定执行权限、不设置安全大小上限，也不替代各 Tool 的 Rust 投影实现和契约 fixture。

## 消费者

| 代号 | 消费者                   | 目的                                     |
| ---- | ------------------------ | ---------------------------------------- |
| M    | Model                    | 当前及未来模型继续行动所需的语义         |
| E    | Event/Renderer           | 用户可见活动、预览、进度和错误           |
| R    | Runtime Extension        | 同一 Run 内扩展状态，如 Skill 激活、Todo |
| T    | Durable Trace            | 长期 Provider-neutral 审计与历史重建     |
| A    | Exact Archive            | 安全捕获原文及完整性元数据               |
| C    | Checkpoint/model-context | 审批恢复和模型当时实际观察过的安全状态   |

M/E/R/T/A/C 是不同投影，不是“先裁一个 JSON 再复用”。字段可同时属于多个消费者，但授权、secret、私有绝对路径和原始二进制不得因复用而越界。

## 主链路

```text
Raw Tool/Core Server/Main result
  -> validate typed identity and closed result schema
  -> Canonical Tool Result
      +-> Archive projection -> Exact Capture/chunks
      +-> Trace projection   -> append-only audit
      +-> Event projection   -> Renderer
      +-> Checkpoint projection -> durable resume/model context
      +-> Runtime extension projection
      `-> Model projection -> shared 10K gate
```

调用参数也有 `model_call`、`trace_call`、`event_call` 和 `checkpoint_call` 投影。模型在当前 Run 可看到自己生成的可修正参数；持久化和 UI 版本必须单独去除秘密、私有内容或不能安全恢复的参数。

## 工具族矩阵

下表描述截至核验日的已注册 Tool 族，包括 Command Session、Skill 安装、内置 Capability、Managed Playwright 和协作 Tool。动态 MCP Server/Managed Playwright Tool 的具体名称来自运行时 catalog，不在本文复制。

| 工具族                                                           | M：行动语义                                                         | E/R：展示或扩展                                                  | T/A/C：持久与恢复                                                       | 特殊规则                                                      |
| ---------------------------------------------------------------- | ------------------------------------------------------------------- | ---------------------------------------------------------------- | ----------------------------------------------------------------------- | ------------------------------------------------------------- |
| `attachments_list*`                                              | scope、分页计数、name/kind/mime/size、可读引用                      | E 保留附件身份和展示信息                                         | T/A/C 保留安全清单页；不暴露宿主路径                                    | opaque cursor 重新校验授权                                    |
| `read_file`                                                      | path、行/字节范围、正文、续读游标、来源截断                         | E 可用有界预览                                                   | T 保存审计页；A 保存捕获正文；C 保存模型页                              | 语义分页优先，避免中央 Gate 跳项                              |
| `read_word` / `read_spreadsheet` / `read_presentation`           | 格式、结构计数、解析文本、截断                                      | E 展示文档元数据                                                 | A 保存安全解析正文；T/C 有界                                            | PDF 不属于旧 `read_pdf` 路径                                  |
| `read_image`                                                     | path/mime/dimensions；像素走原生多模态                              | E 可用安全缩略图                                                 | T/A/C 去除 base64，保留 binary omitted 诊断                             | `image-artifact://` 需 grant                                  |
| `workspace_map`                                                  | summary、treeText、coverage、refine 提示                            | E 可展示统计                                                     | T/A/C 保留有界双视图                                                    | 无组内 cursor，缩小 focusPath                                 |
| `search_files` / `search_code`                                   | matches、范围、计数、nextCursor、遗漏原因                           | E 展示页和 coverage                                              | T/A/C 保留查询身份与页                                                  | cursor 绑定 query/scope/revision                              |
| `web_search`                                                     | answer/snippet/title/url/date/images、Provider continuation 提示    | E 保留 favicon/score/耗时等展示字段                              | T/A/C 保存安全 Provider 页及完整性                                      | 搜索摘要不是网页全文                                          |
| `web_fetch`                                                      | url、正文页、图片、失败和恢复                                       | E/C 使用独立有界正文                                             | T + A 保存安全捕获正文                                                  | `maxChars` 仅兼容 E/C，不限制 A                               |
| `git_diff`                                                       | scope、diff、失败和历史恢复入口                                     | E 展示 diff 状态                                                 | A 保存 spool 文本；T/C 有界                                             | 大结果通过 history open                                       |
| `apply_patch`                                                    | Direct/Staged proposal/result、transaction、操作、revision/conflict | E/R 显示 transient preview、分页 diff、draft/progress 与审批状态 | T/A/C 保存安全 transaction identity、base/target/diff digest 与终态     | 唯一文件 writer；执行权限来自 frozen FileChange proposal      |
| `run_command`                                                    | exit/stdout/stderr 预览、截断、policy 摘要、Artifact observation    | E 展示命令/session/artifact 进度                                 | A 保存一次 exact spool；T/C 保存 frozen request/receipt                 | running 返回 session ID；终态归档一次                         |
| `command_session`                                                | session 状态、增量输出、最新 cursor、终态/unknown                   | E 展示持久 session 生命周期                                      | T/C 保存观察 receipt；**不再次 A**                                      | wait/interrupt；无任意 stdin；authoritative cancel            |
| `office_document` / `office_spreadsheet` / `office_presentation` | documentKind/operation、outputs、进程终态与不确定性                 | E 展示 Office 活动和产物                                         | A 保存 stdout/stderr；T/C 保存 execution/prepared identity 与 outputs   | 模型面仅检查/验证/渲染；render output 走 FileChange 审批      |
| `image_generation`                                               | status/operation、可给 `read_image` 的 URI、failure/visual delivery | E 保留 Artifact 与审计展示，去除 savedPath                       | T/A/C 保留 execution/fingerprint/hash；图片二进制不进文本               | paid side effect unknown 时禁止重放                           |
| Skill resource list/read                                         | URI、资源页、正文或 next cursor                                     | E 显示安全元数据                                                 | T/A/C 绑定 package revision；正文按规则归档                             | `skill://` 是 revision-bound authority                        |
| Skill materialize                                                | 目标、状态、可行动错误                                              | E 显示写入审批/结果                                              | T/A/C 保存 source URI、digest、frozen target                            | FileChange 权限域；批准前不写入                               |
| Skill script preflight/run                                       | runtime/requirements 摘要、argv 结果、stdout/stderr                 | E 展示执行活动                                                   | Run 的输出进入 A；T/C 保存 frozen script/digest                         | 两者均要求 read/write all + Full Access；run 还需每次显式审批 |
| `skills_prepare_install`                                         | inspected metadata、候选、warnings、opaque installRef               | E 可展示来源与预览                                               | T/C 保存安全 inspection identity                                        | 只读，不安装；第三方内容不可信                                |
| `skills_commit_install`                                          | installed/updated/conflict/recovery                                 | E 显示审批与结果                                                 | T/C 保存 frozen install ref/provenance/CAS；必要内容由 Skill store 管理 | 总是显式审批                                                  |
| `skills_activate`                                                | status、Skill name、资源/能力摘要                                   | E/R 产生 `SkillActivated` 与 Toolset change                      | T/C 保存 activation 与 revision                                         | 同一 Turn 激活后更新 dynamic Toolset                          |
| `conversation_history`                                           | view、目录/正文页、open/navigation、完整性                          | E 可展示查询状态                                                 | T/C 只记 query/ref/range/hash；**不再次 A**                             | 防递归归档                                                    |
| `automation_report`                                              | `recorded`、kind、用户可见 summary 的一次性确认                     | E 可展示小型 Tool receipt；无配置编辑能力                        | T/A/C 保存安全 receipt；另由 run-scoped sink 原子写 `automation_runs`   | 仅 Automation HumanRoot；每 Run 最多首次成功一次；无需审批    |
| `activate_capability`                                            | capability/status/恢复建议                                          | E/R 展示批准并触发 toolset change                                | T/C 绑定 activation/manifest/policy                                     | 激活本身不授予未列出的工具                                    |
| Managed Playwright/Capability Tool                               | 安全操作结果、截图 readPath、Browser Artifact refs、分类错误        | E 展示 Main/Core Server-owned 活动                               | T/C 使用 value-free 安全投影；**不进普通 A**                            | 敏感 Tool 绑定 surface/origin 与风险审批                      |
| 外部 MCP Server Tool                                             | 有界 text/structured 结果、binary omitted、outcome                  | E/T/C 只接收安全投影                                             | 原始参数/结果不进普通 A；Checkpoint 使用专用授权边界                    | OutcomeUnknown 不自动 retry                                   |
| 协作 spawn/send/followup/wait/list/interrupt                     | 节点、delivery/wait/interrupt 的安全结果                            | E/R 更新协作树和 mailbox                                         | T/C 保存持久 receipt；**不进 A**                                        | 除 list 外，多数取消需权威结算                                |
| 其他 Runtime Extension（如 Todo）                                | 下一步所需确认、revision 和计数                                     | R/E 接收完整扩展状态                                             | T/C 按扩展契约；A 仅显式 opt-in                                         | 动态定义仍走 10K Gate                                         |

## 关键字段边界

### 文件和 Artifact

- M 可获得 `readPath`/URI 和必要的 scope，不获得受管存储绝对路径。
- E 可获得 Artifact ID、mime、尺寸、页数、展示 URL/URI，不获得 credential、staging path 或生成 Provider 私有响应。
- T/C 保存 hash、revision、Run/call/execution identity，用于审计和幂等。
- A 归档安全文本；图片、Office 二进制和 data URL 只记录省略与内容身份。

### 命令、Office 与脚本

- M 看到 128 KiB/stream 范围内预览、真实来源截断状态和历史恢复入口。
- T/C 需要 policy decision、authorization source、cwd/input binding、runtime fingerprint、expected output 和副作用不确定性。
- E 需要 session、进度、Artifact observation；这些展示字段不能反向授权执行。
- A 从共享 spool 归档一次；`command_session` 轮询不得再复制正文。

### 图像生成

- M 保留可行动的 status、operation、artifact URI、failure 和 visual-input delivery。
- E 保留 execution/audit/Artifact 展示身份，但去除私有 `savedPath`。
- T/C 保存 request fingerprint、profile/adapter revision、provider request identity、阶段和三类不确定性。
- Base64 原始输入/输出不进入任何文本持久投影。

### MCP Server 与 Managed Playwright

- 不通过 Tool 名前缀识别来源，使用 typed identity。
- 模型生成理由可能包含敏感数据；Managed Playwright Event/Trace 使用 Main-owned value-free reason。
- Managed Playwright screenshot 可同时返回经验证的 `image-artifact://` readPath；download 等产物返回 Run 生命周期的 Browser Artifact ref。外部 MCP Server 的原始 resource/binary block 不穿过 Core Server/Main 安全投影边界。

### Scheduled Automation 报告

`automation_report` 的 Tool Result 与业务报告是两条相关但不同的投影：

- canonical Tool Result 是很小的 `recorded/kind/summary` receipt，按默认安全 hooks 进入 M/E/T/A/C，并继续通过共享 10K gate；
- 真正驱动 Automation attention、终态通知和 Run history 的唯一业务消费是 run-scoped `AutomationReportSink` 写入的 `automation_runs.report_kind/result_preview`，不是 Renderer Event、Trace 文本或 Exact Archive 的二次解析；
- repository 只接受绑定了 `agent_run_id`、仍为 `running|waiting_for_approval` 且 `report_kind IS NULL` 的 Run，因此重放 Tool Call 不能覆盖第一次成功报告；
- terminal settlement 保留已写报告和 summary；未报告时才以 assistant 内容生成最多 8192 字节预览，并把 report kind 收敛为 `unknown`。失败文案另限制为 4096 字节。

报告 summary 是用户可见安全文本，不是通知 policy、Task revision 或执行授权。任何消费者都不得根据 summary 关键词推断 `important_update`；只读取闭合的 report kind。

## 不变量

1. Canonical result 只能由执行边界生成；任何 consumer projection 都无执行权。
2. 所有 M 结果最终经过共享 10K token gate。
3. A 在普通 T/M 限长前产生，但不会绕过来源安全限或敏感清洗。
4. T 与持久 model-context 分开，未来代码变化不能改写“模型当时看到什么”。
5. 不归档的 Tool 必须显式 `archives_result=false` 并说明替代权威来源。
6. 未知 Tool 调用持久化时参数 fail closed，不保留任意模型 JSON。
7. Automation 通知与 attention 只消费持久 `report_kind`，不能从 Tool receipt、assistant 文案或 Event 展示字段反推报告语义。

## 代码真源与自动校验

- 投影 hooks：`crates/core/src/tools/mod.rs`
- 集中模型压缩：`crates/core/src/context/model_tool_result_gate.rs`
- Trace normalization：`crates/core/src/conversation_trace.rs`
- Exact Capture：`crates/core/src/exact_capture.rs`
- 契约 fixture：`crates/core/tests/fixtures/tool_result_projection_contract_v1.json`
- 主要断言：`major_tool_result_projections_match_consumer_contract_fixture`
- 各 Tool 的 `*_model_projection`、`*_persistence_projection` 和单元测试
- Automation 报告：`crates/core/src/tools/automation_report.rs`、`crates/core-server/src/application/agent/automation_turn.rs`、`crates/core/src/storage/automation_repository.rs`

## 测试

- `crates/core/tests/fixtures/tool_result_projection_contract_v1.json`
- `major_tool_result_projections_match_consumer_contract_fixture`
- `crates/core/src/runtime/tests/trace_and_projection.rs`
- `crates/core/src/tools/mcp/tests/`
- `crates/core-server/src/application/agent/tests/context_history.rs`
- `crates/core/src/tools/automation_report.rs`
- `crates/core/src/storage/automation_repository/tests.rs`
- `crates/core-server/src/application/automation/scheduler/tests.rs`
- 对应 Renderer/Protocol consumer 测试；字段删除前必须先检索真实消费方

## 变更检查表

- [ ] 新增/删除 Tool 时更新注册契约和本矩阵对应族。
- [ ] 对每个新增字段标注 M/E/R/T/A/C，禁止“暂时全透传”。
- [ ] 私有路径、credential、binary/data URL、模型敏感理由有负向断言。
- [ ] 明确 source truncation、consumer projection 和 model truncation。
- [ ] 需要恢复的字段进入 C；仅展示字段不得误入 C。
- [ ] 不归档 Tool 有一次性权威存储和防递归测试。
- [ ] `automation_report` 变更同时覆盖 receipt 投影、单次 sink 写入、终态 fallback 和通知 policy，不以文案关键词分类。
- [ ] 更新 JSON fixture 与 Renderer/Protocol 消费测试。

## 当前限制

- fixture 重点覆盖主要字段族，尚不是由 Rust 类型自动生成的全量 schema diff。
- 动态 MCP Server/Managed Playwright/Runtime Extension 名称依赖运行时 catalog，本文只维护族级边界。
- 历史旧记录可能缺少新字段；读取兼容不得伪造当时不存在的完整性证明。
- Renderer 仍有少量兼容字段；删除前必须先检索真实消费方并更新协议测试。
- `automation_report` 当前使用默认 Tool projection hooks，尚未进入主要 Tool Result projection fixture 的独立字段族；其 closed schema、单次持久化和通知语义由 Automation 专项测试守护。
