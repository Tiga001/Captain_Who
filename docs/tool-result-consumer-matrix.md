# Tool Result 消费者矩阵与投影契约

状态：Round 3 已实施

日期：2026-07-28

约束：E/R/T/A/C 保持独立消费者契约；M 使用专属语义投影并统一经过固定 10K
Gate。来源捕获、分页、安全硬限和恢复路径见
[Tool Result 上限、投影与恢复契约](tool-result-limits.md)。

## 1. 范围和术语

本文覆盖 Core 静态 Tool、按能力启用的静态 Tool、Runtime Extension 动态 Tool，以及审批后由 Host 执行的 Tool。

消费者：

- **M — Model**：当前统一模型历史中的 Tool result observation。
- **E — Renderer/Event**：`AgentEvent::ToolResult` 或专用展示事件。
- **R — Runtime Extension**：同一 Run 中消费结果的 Extension。
- **T — Durable Trace**：有界、可审计的 Trace 投影。
- **A — Exact Archive**：安全清洗后、不受模型长度投影影响的精确归档。
- **C — Checkpoint**：审批暂停、恢复和幂等结算所需的检查点。

信息归属：

- **行动**：模型判断结果和下一步所需。
- **UI**：状态、卡片、导航或预览所需。
- **审计**：诊断、可追溯性和结果真实性所需。
- **恢复**：审批恢复、幂等、冲突检查和重启恢复所需。
- **正文**：需逐字归档并可由 `conversation_history` 恢复的正文。

`AgentToolResult` 公共信封字段的消费者固定如下：

| 字段     | 消费者         | 信息归属                                                               |
| -------- | -------------- | ---------------------------------------------------------------------- |
| `callId` | 协议/E/R/T/A/C | 恢复、幂等、调用关联；模型 provider 协议携带，不重复写入 observation   |
| `tool`   | 协议/E/R/T/A/C | 路由、展示、审计；模型 provider 协议携带，不重复写入 observation       |
| `ok`     | 协议/E/R/T/A/C | 行动、UI、审计、恢复；模型失败语义由 provider error 标记和精简负载表达 |
| `error`  | M/E/R/T/A/C    | 行动、UI、审计、恢复                                                   |
| `result` | 见下方矩阵     | Tool 自有负载                                                          |

矩阵中的 `x.*` 表示该对象当前的全部嵌套字段。字段组用于压缩文档篇幅，不授权删除其中任何字段。

## 2. 当前投影链路

```text
Raw AgentToolResult
├─ archive_projection ───────────────→ Exact Archive
├─ model_projection ─────────────────→ Model History
├─ trace_projection ──→ 有界化 ─────→ Durable Trace
├─ checkpoint_projection ────────────→ Approval Checkpoint
├─ event_projection ─→ event redact ─→ Renderer/Event
└─ trace_projection ─────────────────→ Runtime Extension on_event
```

Round 2 已为高成本和多消费者 Tool 增加专属 Model Projection：

- Command、Office、Web、Workspace、附件、文件/文档读取和代码搜索只向模型保留行动字段、正文、状态与分页路由。
- Todo 和 Skill 激活结果只确认接受状态与必要摘要；完整状态由 Runtime Extension 和专用事件消费。
- `read_image`/`image_generation` 的二进制、多媒体展示、Artifact 审计继续走各自既有投影。
- `conversation_history` 保留 `open`/`navigation` 和历史正文，移除固定说明、统计重复值、后端 ID/hash/时间戳。
- `write_file` 保留下一步所需的 draft ID、路径、状态、游标和 tail，移除会话身份、时间戳与预算常量。
- 审批恢复使用与正常执行相同的纯 Model Projection；Checkpoint 仍保存完整 Canonical Result。

Tool observation 文本现在只序列化精简后的 `result`/`error`，不再重复 `type/tool/callId/ok`、固定英文前言或 Markdown JSON 代码块。

## 3. 完整 Tool 清单

| 类别           | Tool                                                                                                                            |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| 附件与读取     | `attachments_list`、`attachments_list_project`、`read_file`、`read_image`、`read_word`、`read_presentation`、`read_spreadsheet` |
| 工作区检索     | `workspace_map`、`search_files`、`search_code`、`git_diff`                                                                      |
| Web            | `web_search`、`web_fetch`                                                                                                       |
| 文件与命令     | `apply_patch`、`write_file`、`run_command`                                                                                      |
| Skill          | `skills_list_resources`、`skills_read_resource`、`skills_materialize_resource`、`skills_preflight_script`、`skills_run_script`  |
| 历史与 Goal    | `conversation_history`、`get_goal`、`create_goal`、`update_goal`                                                                |
| 条件能力       | `office_document`、`office_spreadsheet`、`office_presentation`、`image_generation`                                              |
| 动态 Extension | `todo_update`、`skills_activate`                                                                                                |

激活 Skill 后披露的应用自有 Tool 仍走同一 `ToolRegistry` 投影管线。其 Schema 绑定 Skill revision，注册新 Tool 时必须同时增加消费者契约，不能在本文件中虚构固定结果字段。模型可在同一响应中同时调用 `skills_activate` 与该请求已经披露的其他 Tool；这些 sibling 调用按生成响应时冻结的 ToolSet、权限、Approval 与 Sandbox 契约正常执行。完整 Skill 指令和新解锁 Tool 只从下一次模型请求生效，同响应中猜测新 Tool 仍按“当前未暴露”拒绝。若该批次在后续 sibling 进入 Approval，v9 checkpoint 会分别保留请求边界的 capability/ToolSet 与已经发生的 extension 效果；恢复先在旧 ToolSet 下结算整批，下一模型边界才采用激活后的新能力。DeepSeek 等 exact-grouped Provider 恢复时，queued sibling 还会重绑定到已认证 Provider Turn 的统一交换分组。该语义不随 Provider profile 改变。

审批 Host action 与结果：

| `AgentProposedAction`  | Tool                          | Host 结果                         |
| ---------------------- | ----------------------------- | --------------------------------- |
| `Diff`                 | `apply_patch`                 | `AgentPatchResult`                |
| `FileWrite`            | `write_file.finish`           | `AgentFileWriteResult`            |
| `Command`              | `run_command`                 | `AgentCommandExecutionResult`     |
| `SkillMaterialization` | `skills_materialize_resource` | `AgentSkillMaterializationResult` |
| `SkillScript`          | `skills_run_script`           | `AgentSkillScriptResult`          |
| `OfficeOperation`      | 三个 Office Tool              | `OfficeExecutionResult`           |
| `ToolCall`             | 通用拒绝/兼容分支             | structured unsupported/rejected   |

审批恢复使用 Checkpoint 中的原始模型 ToolCall，不能从 Host action 反向合成新的模型调用。

## 4. 字段消费者矩阵

### 4.1 附件与读取

| Tool                | 字段                                                                                       | 消费者       | 归属和约束                                                          |
| ------------------- | ------------------------------------------------------------------------------------------ | ------------ | ------------------------------------------------------------------- |
| `attachments_list*` | `scope`、`total/returned/omitted`、`truncated/nextCursor`                                  | M/E/T/A/C    | 行动、UI、审计、稳定分页；cursor 绑定 conversation/project 权限范围 |
| `attachments_list*` | `scopeNote`、`library.*`、`attachments[].id/messageId`                                     | E/T/A/C      | UI、恢复；Renderer 用于定位 timeline 附件                           |
| `attachments_list*` | `conversationId`、`projectId`、`createdAt`                                                 | E/T/A/C      | 审计、恢复；默认不进入模型                                          |
| `attachments_list*` | `name`、`kind`、`mimeType`、`sizeBytes`、`readPath`                                        | M/E/T/A/C    | 行动、UI、恢复；`readPath` 是权威读取路由                           |
| `read_file`         | `path`、`revision`                                                                         | M/E/T/A/C    | 行动、一致性、恢复                                                  |
| `read_file`         | `startLine`、`startByte`、`endLine`、`endByteExclusive`                                    | M/E/T/A/C    | 行动、分页、审计                                                    |
| `read_file`         | `startColumn`、`endColumn`、`returnedBytes`、`estimatedContentTokens`、`outputTokenBudget` | E/T/A/C      | UI、审计；默认模型投影不携带内部预算                                |
| `read_file`         | `totalLines`、`totalBytes`                                                                 | M/E/T/A/C    | 行动、分页、审计                                                    |
| `read_file`         | `truncated`、`truncatedReason`、`nextStartByte`、`nextStartLine`                           | M/E/T/A/C    | 行动、恢复；禁止静默截断                                            |
| `read_file`         | `nextStartColumn`                                                                          | E/T/A/C      | 兼容 UI/审计；模型续读使用权威 byte cursor                          |
| `read_file`         | `content`                                                                                  | M/E/T/A/C    | 行动、正文                                                          |
| 文档读取            | `path`、`format`、页/部件/工作表/幻灯片数量、捕获完整性字段、`truncated`                   | M/E/T/A/C    | 行动、UI、审计                                                      |
| 文档读取            | `sizeBytes`、`extractor`                                                                   | E/T/A/C      | UI、解析审计；不进入默认模型投影                                    |
| 文档读取            | `text`                                                                                     | M/E/T/A/C    | 行动、正文；超限后由 Archive `historyOpen` 恢复                     |
| `read_image`        | `path`、`format`、`mimeType`、`sizeBytes`                                                  | M/E/T/A/C    | 行动、UI、审计、恢复                                                |
| `read_image`        | `source.*`、`sha256`                                                                       | E/T/A/C      | 来源身份与审计；不重复进入模型文本                                  |
| `read_image`        | 生成物 `artifact.*`                                                                        | E/T/A/C      | Host Artifact 原图预览、内容身份与恢复；不进入模型文本              |
| `read_image`        | `image.mimeType`、`image.dataBase64`                                                       | 原始 Runtime | 原生多模态输入；不进入文本、事件、Trace、Archive、Checkpoint        |
| `read_image`        | `thumbnailDataUrl`                                                                         | E            | 有界 UI 缩略图                                                      |
| `read_image`        | `binaryOmittedFromHistory`、`thumbnailOmittedFromEvent`                                    | T/A/C 或 E   | 审计；明确二进制投影省略                                            |

`read_image` 的模型输入契约只暴露一个必填 `path`。工作区相对/绝对路径、系统别名、
`@attachments/...`、`image-artifact://...` 和 revision-bound `skill://...` 均由后端自动路由；
旧 `source`/`filePath` 只保留执行兼容，不再进入模型 Tool Schema。简单输入不会扩大权限：
外部绝对路径仍要求 `read=all`，附件、Skill 和生成物仍分别复验其权威记录。

### 4.2 工作区检索

| Tool            | 字段                                                                                             | 消费者    | 归属和约束                                              |
| --------------- | ------------------------------------------------------------------------------------------------ | --------- | ------------------------------------------------------- |
| `workspace_map` | `summary.*`、`treeText`                                                                          | M/E/T/A/C | 行动、UI、审计                                          |
| `workspace_map` | `workspace.*`、`tree[]`                                                                          | E/T/A/C   | UI、审计；模型只接收紧凑 `treeText`                     |
| `workspace_map` | `coverage.*`、`refine`、`truncated.walk/tree`                                                    | M/E/T/A/C | 完整性边界；缩小 focusPath 恢复，不提供 cursor          |
| `search_files`  | `matches[].path/kind/sizeBytes`、`total/returned/omitted`、`truncated/nextCursor`                | M/E/T/A/C | 行动、审计、opaque cursor 分页                          |
| `search_code`   | `matches[].path/lineNumber/line`、单行省略说明、`total/returned/omitted`、`truncated/nextCursor` | M/E/T/A/C | 行动、正文、opaque cursor 分页；超长行用 read_file 恢复 |
| 搜索工具        | `query`                                                                                          | E/T/A/C   | UI、审计；ToolCall 已携带 query，模型结果不重复         |
| `git_diff`      | `path`、`patch`、`truncated`                                                                     | M/E/T/A/C | 行动、UI、正文                                          |

### 4.3 Web

| Tool         | 字段                                                                                      | 消费者    | 归属和约束                                                       |
| ------------ | ----------------------------------------------------------------------------------------- | --------- | ---------------------------------------------------------------- |
| `web_search` | `answer`、`results[].title/url/content/publishedDate`、`contentKind/fullContentTool`      | M/E/T/A/C | Provider 搜索摘要；不是网页全文                                  |
| `web_search` | `query`                                                                                   | E/T/A/C   | UI、审计；ToolCall 已携带 query                                  |
| `web_search` | Provider `cursor/next/nextCursor/continueWith`、`sourceCompleteness` 与显式来源完整性字段 | M/E/T/A/C | 原样保留；未声明时写 `unknown`，不伪造 `truncatedAtSource=false` |
| `web_search` | `results[].score`                                                                         | E/T/A/C   | UI 相关性排序、审计                                              |
| `web_search` | `results[].favicon`                                                                       | E/T/A/C   | UI；Renderer 来源卡片消费                                        |
| `web_search` | `provider`、`responseTime`                                                                | E/T/A/C   | UI、审计                                                         |
| `web_search` | `images[]`                                                                                | M/E/T/A/C | 行动、UI、证据；用途确认前不删除                                 |
| `web_fetch`  | `url`、`content`、coverage、`truncated` 与来源完整性字段                                  | M/E/T/A/C | 行动、UI、正文                                                   |
| `web_fetch`  | `requestedUrl`、`favicon`                                                                 | E/T/A/C   | UI；Renderer 来源卡片消费                                        |
| `web_fetch`  | `provider`、`format`、`extractDepth`、`responseTime`                                      | E/T/A/C   | UI、审计                                                         |
| `web_fetch`  | `images[]`、`failedResults[]`                                                             | M/E/T/A/C | 页面证据、失败诊断                                               |

`favicon` 的真实消费者位于 `agentWebSearch.ts` 和 `WebSearchSources.tsx`。它不是无效字段，只是不需要默认进入模型文本。

### 4.4 Todo、Goal 与历史

| Tool                               | 字段                                                                                           | 消费者            | 归属和约束                                           |
| ---------------------------------- | ---------------------------------------------------------------------------------------------- | ----------------- | ---------------------------------------------------- |
| `todo_update` Model                | `accepted/revision/itemCount/completedCount`                                                   | M                 | 简短确认；不重复整份 Todo                            |
| `todo_update` Canonical            | `revision/updatedAt`、`items[].id/title/status/note/createdAt/updatedAt`                       | E/R/T/A/C         | UI、审计、恢复；R 生成 Todo context 和 `TodoUpdated` |
| Goal 原始结果                      | `goal.objective/status`、`created`、`updated`                                                  | M，当前也进入 E/C | 行动、恢复                                           |
| Goal Trace                         | `goalPresent`、`created`、`updated`、`status`、`goalStatePersisted`                            | T/E               | 审计；不进入 Exact Archive                           |
| `conversation_history` Model       | `view`、turn/result/timeline/record、`content`、`navigation.*`、`open`、范围、hash、完整性标记 | M/C               | 行动、正文、恢复；固定 instruction 不进入结果        |
| `conversation_history` Trace/Event | query/open ref、读取范围、hash、返回数量、状态                                                 | T/E               | 审计；不重复保存取回正文，不再次归档                 |

Todo 的完整状态不能从 E/R 删除；当前 M 已经只接收确认回执，权威 Todo snapshot
继续由 Extension 注入。

### 4.5 文件、Patch 与命令

| Tool                        | 字段                                                                                                  | 消费者                               | 归属和约束                                         |
| --------------------------- | ----------------------------------------------------------------------------------------------------- | ------------------------------------ | -------------------------------------------------- |
| `apply_patch`               | `status`、`operation`、`filePath`、`appliedFilePaths[]`                                               | M/E/T/A/C                            | 行动、UI、审计、恢复                               |
| `apply_patch`               | `gitDiff.patch/truncated`                                                                             | M/E/T/A/C                            | 行动、UI、正文                                     |
| `apply_patch`               | `gitDiffError`、`error`、`message`                                                                    | M/E/T/A/C                            | 冲突诊断、恢复                                     |
| `write_file` Model 草稿     | `draft.draftId/filePath/mode/status/lineCount/byteCount/chunkCount/nextChunkIndex/statsFinal/summary` | M                                    | 行动；`draftId` 是权威后续路由                     |
| `write_file` Canonical 草稿 | conversation/project/baseRevision/additions/deletions/时间等完整 `draft.*`                            | E/R/T/A/C                            | UI、审计、恢复                                     |
| `write_file` 草稿           | `tail`                                                                                                | M/T/A/C                              | 私有草稿正文；事件明确删除                         |
| `write_file` 草稿           | `transactionState`、`requiresFinishBeforeResponse`、`nextAction`                                      | M/E/T/A/C                            | 行动、恢复                                         |
| `write_file` 草稿           | `maxDraftBytes`                                                                                       | E/T/A/C                              | UI/后端预算审计；不进入模型                        |
| `write_file` Host 终态      | `status`、`draftId`、`mode`、`filePath`、统计、`revision`、`error`、`message`                         | M/E/T/A/C                            | 权威写入回执                                       |
| `run_command`               | `command`、`cwd`                                                                                      | E/T/A/C                              | UI、审计；ToolCall 已携带，不在结果中重复给模型    |
| `run_command`               | `exitCode`、`stdout`、`stderr`、`timedOut`、`cancelled`、截断和 `error`                               | M/E/T/A/C                            | 行动、UI、审计、正文                               |
| `run_command`               | `durationMs`                                                                                          | E/T/A/C                              | UI、审计                                           |
| `run_command`               | `policyEvaluation.*`                                                                                  | E/T/A/C；M 仅 `decision/code/reason` | 安全审计、恢复                                     |
| `run_command`               | `artifactObservation.status/partial/stopReasons/scanned/returned/omitted` 与精简 `changes[]`          | M/E/T/A/C                            | 行动、UI、审计、恢复；partial 结果不能作为完整证据 |
| `run_command`               | `artifactObservation.schemaVersion/coverage.*`、`expectedOutputs[]`、`warnings[]`、完整变更详情       | E/T/A/C                              | UI、审计、恢复；模型不承担底层扫描细节             |
| `run_command`               | `inputFiles[].mountPath/sourceKind/sizeBytes/sha256`、`runtime.*`                                     | E/T/A/C                              | 审计、恢复；冻结输入和托管 runtime 证据            |
| Host 审计失败包装           | `type/code/recovery/phase`、执行与副作用不确定性、`auditError`、`reconciliationError`、`execution.*`  | M/E/T/A/C                            | 行动、审计、恢复；防止有副作用后盲目重试           |

Renderer 使用 `artifactObservation` 构建 Office Artifact 卡片；失败 observation 仍保留作审计，但不能变成成功卡片。

### 4.6 Office

三个 Office Tool 共用 `OfficeExecutionResult`：

| 字段组                                                                                             | 消费者    | 归属和约束                                                              |
| -------------------------------------------------------------------------------------------------- | --------- | ----------------------------------------------------------------------- |
| `documentKind`、`operation`                                                                        | M/E/T/A/C | 行动、审计、恢复                                                        |
| `providerId`、`engineRevision`、`argv`、`cwd`                                                      | E/T/A/C   | UI、审计、恢复、幂等                                                    |
| `exitCode`、`stdout`、`stderr`、超时/取消/截断/错误与捕获完整性字段                                | M/E/T/A/C | 行动、UI、审计、正文                                                    |
| `durationMs`                                                                                       | E/T/A/C   | UI、审计                                                                |
| `outputs[].role/kind/mimeType/readPath/scope/readableByAgent/sizeBytes/width/height/pageSelection` | M/E/T/A/C | 后续读取、UI Artifact、恢复                                             |
| `outputs[].layoutCoverage.requestedPages/evidence/grid.*`                                          | M/E/T/A/C | 仅证明冻结页集按可信渲染器布局完整落入 PNG viewport；不证明逐页视觉内容 |
| `outputs[].source/sha256`                                                                          | E/T/A/C   | 审计、恢复                                                              |
| 审计失败包装 `execution.*` 和不确定性字段                                                          | M/E/T/A/C | 行动、审计、恢复                                                        |

`skillOfficeActivity.ts` 同时消费 Office 输出和 Command Artifact Observation，E 投影必须兼容。

### 4.7 Skill 资源和脚本

| Tool                          | 字段                                                                                                    | 消费者    | 归属和约束                                         |
| ----------------------------- | ------------------------------------------------------------------------------------------------------- | --------- | -------------------------------------------------- |
| `skills_activate` Model       | `status`、`skill.name/hasResources`                                                                     | M         | 简短确认，不重复后端身份                           |
| `skills_activate` Canonical   | `schemaVersion/status/activatedBy/activationRevision/reason`、完整 `skill.*`                            | E/R/T/A/C | UI、恢复；R 用 `status` 和 `skill.id` 更新 ToolSet |
| `skills_list_resources`       | `rootUri`、`resources[].uri/path/kind/byteLength/contentDigest`、`truncated`、`nextAfterPath`           | M/E/T/A/C | 行动、审计、恢复                                   |
| `skills_read_resource`        | `uri`、byte 范围、长度、`truncated`、`nextStartByte`                                                    | M/E/T/A/C | 行动、分页、恢复                                   |
| `skills_read_resource`        | `content`                                                                                               | M/A/C     | 行动、正文；T/E 用 `contentOmittedFromHistory`     |
| `skills_materialize_resource` | `status/sourceUri/sourcePrefix/destination/sourceRevision/fileCount/byteCount/planDigest/error/message` | M/E/T/A/C | 行动、UI、审计、幂等                               |
| `skills_preflight_script`     | script/skill/resource 身份、`ready`、`resourceDigest`、`preflight.*`                                    | M/E/T/A/C | 行动、审计、恢复                                   |
| `skills_run_script`           | script/skill/revision/digest、`preflight.*`、进程结果和截断/错误字段                                    | M/E/T/A/C | 行动、UI、审计、正文、恢复                         |

### 4.8 图片生成

| 字段                                                                                                                                                     | 消费者       | 归属和约束                                           |
| -------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------ | ---------------------------------------------------- |
| `schemaVersion`、`status`、`operation`                                                                                                                   | M/E/T/A/C    | 行动、UI、审计、恢复                                 |
| 派生的 `path`（`artifact.uri` 的模型友好副本）                                                                                                           | M/T/A/C      | 可直接作为 `read_image.path`，历史恢复时保持同一语义 |
| `artifact.kind/format/mimeType/width/height/sizeBytes`                                                                                                   | M/E/T/A/C    | 行动、UI、审计、恢复                                 |
| `artifact.uri`                                                                                                                                           | M/E/T/A/C    | Artifact 身份及兼容消费                              |
| `artifact.artifactId/sha256`                                                                                                                             | E/T/A/C      | 内容身份、UI、审计、恢复                             |
| `audit.executionId/requestFingerprint/providerProfileId/adapterId/profileRevision/modelId/providerRequestId/httpStatus/createdAt/completedAt/durationMs` | E/T/A/C      | UI、审计、恢复、幂等                                 |
| `failure.code/phase/message/recovery/retryable` 与三个不确定性布尔值                                                                                     | M/E/T/A/C    | 行动、UI、审计、恢复                                 |
| 派生的 `visualInputDelivery`                                                                                                                             | M            | 仅描述生成当时的视觉投递                             |
| `savedPath`                                                                                                                                              | M/T/A/C      | 复制/导出与后端恢复；E 明确去除                      |
| 原始 `visualInputStatus`                                                                                                                                 | T/A/C        | 原始运行审计；E 和模型投影去除                       |
| `image.mimeType/dataBase64`                                                                                                                              | 原始 Runtime | 原生多模态输入；所有文本投影去除                     |
| `binaryOmittedFromHistory`                                                                                                                               | T/A/C        | 二进制历史省略审计                                   |

图片 audit 被 Core Server action audit、启动恢复和 Renderer 图片活动共同消费，不能从原始/持久结果删除。

## 5. Round 3 收敛结果

已删除或收敛：

- `web_search` 的 answer 1,200 字符和 result content 600 字符本地模型裁剪；
- 文档读取在 Exact Archive 前的 40K/120K 字符裁剪；
- `web_fetch` 旧“模型正文字符上限”语义；保留的 `maxChars` 仅是
  Event/Checkpoint 兼容投影；
- `git_diff` 在 Archive 前的约 200 KiB 裁剪；
- Command、Office CLI、Skill Script 各自捕获 stdout/stderr 的重复实现；
- 搜索和附件列表各自暴露内部 offset/path 的继续参数；统一为 opaque cursor；
- 仅凭 `truncated` 猜测来源丢失的逻辑；Source、分页、消费者投影和 Model Gate
  截断分别记录。

继续保留：

- 文件大小、解压、媒体、解析、传输、进程内存、walk、哈希和时间安全限；
- Renderer favicon、Todo、附件定位、文件草稿和 Artifact Observation 字段；
- Trace/Archive/Checkpoint 的审计、恢复和幂等身份；
- 旧会话缺少新 cursor 字段时的读取兼容。

## 6. 契约测试

新增夹具 `crates/core/tests/fixtures/tool_result_projection_contract_v1.json`，覆盖 Web favicon/score/正文、附件身份与 readPath、Workspace Map 双视图、Command policy 与 Artifact Observation、File Draft tail、Skill Resource 正文生命周期。

新增或加强：

- `major_tool_result_projections_match_consumer_contract_fixture`：逐字段断言 M/E/T/A/C 五个投影。
- `todo_tool_updates_state_and_extension_emits_event`：Todo id、时间、状态和 revision 到达 R/E。
- `agentWebSearch.test.ts`：Renderer 实际消费 favicon、score、publishedDate、responseTime 和 truncated。

继续依赖的既有契约：

- `read_image.rs`：图片二进制、缩略图和历史省略标记。
- `image_generation.rs`、`ImageGenerationActivity.browser.test.tsx`：Artifact、audit 和私有字段边界。
- `core-server/src/agent/tests/terminal_events.rs`：Command Artifact Observation 的持久审计。
- `skillOfficeActivity.test.ts`、`SkillOfficeTimeline.browser.test.tsx`：Artifact/Office 到 Renderer。
- `runtime/tests.rs`：write-file redaction、审批恢复和 Exact Archive。

若后续有意修改投影，必须先更新本矩阵，再只修改对应 stage 的夹具断言。未同步更新契约的字段删除应由测试阻止。

## 7. Round 3 模型语义投影

第 4 节记录 Raw Result 的完整消费者归属；下表是当前实际送入 M 的白名单。未列出的同组字段仍只属于 E/R/T/A/C。

| Tool                                 | 当前 M 投影                                                                                                                             |
| ------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------- |
| `attachments_list*`                  | scope、total/returned/omitted、truncated/nextCursor、附件 name/kind/mimeType/sizeBytes/readPath                                         |
| `read_file`                          | path、行/总量、content、truncated/reason、next cursor                                                                                   |
| `read_word/presentation/spreadsheet` | path/format、部件/幻灯片/工作表计数、text、truncated                                                                                    |
| `read_image`                         | path/format/mimeType/sizeBytes；像素另走原生多模态消息                                                                                  |
| `workspace_map`                      | summary、treeText、coverage/refine、truncated；不重复发送 tree/workspace 参数                                                           |
| `search_files/search_code`           | total/returned/omitted、matches、truncated/nextCursor、来源遗漏说明；不重复发送 query                                                   |
| `web_search`                         | Provider answer/snippets、title/url/publishedDate、images、contentKind/fullContentTool 和 Provider 恢复字段                             |
| `web_fetch`                          | url、content、images、失败摘要、truncated                                                                                               |
| `write_file`                         | 可行动 draft 摘要、tail、transactionState、requiresFinish、nextAction；Host 终态只保留状态/路径/统计/错误                               |
| `run_command`                        | exit/stdout/stderr/真实截断与失败状态、精简 policy、Artifact partial/stopReasons/scanned/returned/omitted/changes/expected outputs      |
| 三个 Office Tool                     | documentKind/operation、可复用 outputs、进程结果与失败不确定性                                                                          |
| Skill 资源/脚本 Tool                 | URI/游标/正文或执行结果、精简 preflight；去除 revision/digest/runtime fingerprint                                                       |
| `skills_activate`                    | status、Skill name/hasResources；完整激活记录由 Extension 消费                                                                          |
| `image_generation`                   | status/operation、可直接交给 `read_image` 的 path、Artifact 展示元数据、savedPath、failure/visualInputDelivery；去除 audit/hash/后端 ID |
| `conversation_history`               | view、语义目录/正文、open/navigation、范围和截断；去除固定说明、计数重复、后端 ID/hash/时间戳                                           |
| `todo_update`                        | accepted、revision、itemCount、completedCount；完整 Todo 由 request-only context 和 Renderer event 消费                                 |

旧 PDF 专用读取工具及其 Protocol、Renderer 和 Timeline 兼容分支已完全删除；PDF
处理统一通过内置 PDF Skill、`run_command` 和 `read_image` 完成。

所有失败投影还会统一保留 `code/errorCode/recovery/phase`、权限/能力要求和副作用不确定性；重复的 `message/error` 只保留一份。

## 8. 结论

模型语义投影已经与 UI、审计、精确归档和审批恢复解耦。模型只承担完成下一步所需的内容成本；Renderer 仍获得 favicon、Todo、附件身份和展示统计，Trace/Archive/Checkpoint 仍保存原有审计与恢复字段。契约夹具会阻止后续精简误删非模型消费者依赖。
