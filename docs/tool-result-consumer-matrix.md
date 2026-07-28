# Tool Result 消费者矩阵与投影契约

状态：Round 1 基线  
日期：2026-07-28  
约束：本轮只确认消费者、建立契约与测试，不改变 Tool 的执行、模型输入、事件、持久化或恢复行为。

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

| 字段     | 消费者      | 信息归属             |
| -------- | ----------- | -------------------- |
| `callId` | M/E/R/T/A/C | 恢复、幂等、调用关联 |
| `tool`   | M/E/R/T/A/C | 路由、展示、审计     |
| `ok`     | M/E/R/T/A/C | 行动、UI、审计、恢复 |
| `error`  | M/E/R/T/A/C | 行动、UI、审计、恢复 |
| `result` | 见下方矩阵  | Tool 自有负载        |

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

多数 Tool 目前仍使用 canonical clone。已有专属投影：

- `read_image`：二进制仅用于原生多模态输入；缩略图仅进入事件；历史去除图片正文。
- `image_generation`：模型、事件和历史分别去除不同的运行时图片字段。
- `skills_read_resource`：Trace/Event 去除 `content`，Model/Archive/Checkpoint 保留。
- `conversation_history`：Trace/Event 只保存查询、引用、范围、hash 和状态，避免递归保存历史正文。
- Goal Tool：Trace 只保存持久化确认，结果不进入 Exact Archive。
- `write_file`：发布事件前额外移除私有 `tail`。

后续精简应只改变相应 projection，不直接修改原始 Host 结果。

## 3. 完整 Tool 清单

| 类别           | Tool                                                                                                                                        |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| 附件与读取     | `attachments_list`、`attachments_list_project`、`read_file`、`read_image`、`read_pdf`、`read_word`、`read_presentation`、`read_spreadsheet` |
| 工作区检索     | `workspace_map`、`search_files`、`search_code`、`git_diff`                                                                                  |
| Web            | `web_search`、`web_fetch`                                                                                                                   |
| 文件与命令     | `apply_patch`、`write_file`、`run_command`                                                                                                  |
| Skill          | `skills_list_resources`、`skills_read_resource`、`skills_materialize_resource`、`skills_preflight_script`、`skills_run_script`              |
| 历史与 Goal    | `conversation_history`、`get_goal`、`create_goal`、`update_goal`                                                                            |
| 条件能力       | `office_document`、`office_spreadsheet`、`office_presentation`、`image_generation`                                                          |
| 动态 Extension | `todo_update`、`skills_activate`                                                                                                            |

激活 Skill 后披露的应用自有 Tool 仍走同一 `ToolRegistry` 投影管线。其 Schema 绑定 Skill revision，注册新 Tool 时必须同时增加消费者契约，不能在本文件中虚构固定结果字段。

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

| Tool                | 字段                                                                                       | 消费者                | 归属和约束                                                   |
| ------------------- | ------------------------------------------------------------------------------------------ | --------------------- | ------------------------------------------------------------ |
| `attachments_list*` | `scope`、`scopeNote`、`library.*`、`total`、`truncated`                                    | M/E/T/A/C             | 行动、UI、审计；`total` 是 UI 计数                           |
| `attachments_list*` | `attachments[].id`、`messageId`                                                            | E/T/A/C，当前也进入 M | UI、恢复；Renderer 用于定位 timeline 附件                    |
| `attachments_list*` | `conversationId`、`projectId`、`createdAt`                                                 | T/A/C，当前也进入 M/E | 审计、恢复；仅是模型投影候选                                 |
| `attachments_list*` | `name`、`kind`、`mimeType`、`sizeBytes`、`readPath`                                        | M/E/T/A/C             | 行动、UI、恢复；`readPath` 是权威读取路由                    |
| `read_file`         | `path`、`revision`                                                                         | M/E/T/A/C             | 行动、一致性、恢复                                           |
| `read_file`         | `startLine`、`startColumn`、`startByte`、`endLine`、`endColumn`、`endByteExclusive`        | M/T/A/C，当前也进入 E | 行动、分页、审计                                             |
| `read_file`         | `totalLines`、`totalBytes`、`returnedBytes`、`estimatedContentTokens`、`outputTokenBudget` | M/T/A/C，当前也进入 E | 行动、审计；token 计量可从 M 精简                            |
| `read_file`         | `truncated`、`truncatedReason`、`nextStartByte`、`nextStartLine`、`nextStartColumn`        | M/E/T/A/C             | 行动、恢复；禁止静默截断                                     |
| `read_file`         | `content`                                                                                  | M/T/A/C，当前也进入 E | 行动、正文                                                   |
| `read_pdf`          | `path`、`format`、`sizeBytes`、`pageCount`、`truncated`                                    | M/E/T/A/C             | 行动、UI、审计                                               |
| `read_pdf`          | `text`                                                                                     | M/T/A/C，当前也进入 E | 行动、正文                                                   |
| `read_word`         | `path`、`format`、`sizeBytes`、`extractor`、`partCount`、`truncated`                       | M/E/T/A/C             | 行动、审计                                                   |
| `read_word`         | `text`                                                                                     | M/T/A/C，当前也进入 E | 行动、正文                                                   |
| `read_presentation` | `path`、`format`、`sizeBytes`、`extractor`、`slideCount`、`truncated`                      | M/E/T/A/C             | 行动、审计                                                   |
| `read_presentation` | `text`                                                                                     | M/T/A/C，当前也进入 E | 行动、正文                                                   |
| `read_spreadsheet`  | `path`、`format`、`sizeBytes`、`extractor`、`sheetCount`、`truncated`                      | M/E/T/A/C             | 行动、审计                                                   |
| `read_spreadsheet`  | `text`                                                                                     | M/T/A/C，当前也进入 E | 行动、正文                                                   |
| `read_image`        | `path`、`source.*`、`format`、`mimeType`、`sizeBytes`、`sha256`                            | M/E/T/A/C             | 行动、UI、审计、恢复                                         |
| `read_image`        | `image.mimeType`、`image.dataBase64`                                                       | 原始 Runtime          | 原生多模态输入；不进入文本、事件、Trace、Archive、Checkpoint |
| `read_image`        | `thumbnailDataUrl`                                                                         | E                     | 有界 UI 缩略图                                               |
| `read_image`        | `binaryOmittedFromHistory`、`thumbnailOmittedFromEvent`                                    | T/A/C 或 E            | 审计；明确二进制投影省略                                     |

### 4.2 工作区检索

| Tool            | 字段                                                   | 消费者    | 归属和约束                               |
| --------------- | ------------------------------------------------------ | --------- | ---------------------------------------- |
| `workspace_map` | `workspace.*`、`summary.*`                             | M/E/T/A/C | 行动、UI、审计                           |
| `workspace_map` | `tree[]`、`treeText`                                   | M/E/T/A/C | 行动、正文；两种表示重复，是明确精简候选 |
| `workspace_map` | `truncated.walk`、`truncated.tree`                     | M/E/T/A/C | 完整性边界                               |
| `search_files`  | `query`、`matches[].path/kind/sizeBytes`、`truncated`  | M/E/T/A/C | 行动、审计                               |
| `search_code`   | `query`、`matches[].path/lineNumber/line`、`truncated` | M/E/T/A/C | 行动、正文                               |
| `git_diff`      | `path`、`patch`、`truncated`                           | M/E/T/A/C | 行动、UI、正文                           |

### 4.3 Web

| Tool         | 字段                                                                        | 消费者                | 归属和约束                       |
| ------------ | --------------------------------------------------------------------------- | --------------------- | -------------------------------- |
| `web_search` | `query`、`answer`、`results[].title/url/content/publishedDate`、`truncated` | M/E/T/A/C             | 行动、UI、正文                   |
| `web_search` | `results[].score`                                                           | E/T/A/C，当前也进入 M | UI 相关性排序、审计              |
| `web_search` | `results[].favicon`                                                         | E/T/A/C，当前也进入 M | UI；Renderer 来源卡片消费        |
| `web_search` | `provider`、`responseTime`                                                  | E/T/A/C，当前也进入 M | UI、审计                         |
| `web_search` | `images[]`                                                                  | M/E/T/A/C             | 行动、UI、证据；用途确认前不删除 |
| `web_fetch`  | `url`、`requestedUrl`、`content`、`truncated`                               | M/E/T/A/C             | 行动、UI、正文                   |
| `web_fetch`  | `favicon`                                                                   | E/T/A/C，当前也进入 M | UI；Renderer 来源卡片消费        |
| `web_fetch`  | `provider`、`format`、`extractDepth`、`responseTime`                        | E/T/A/C，当前也进入 M | UI、审计                         |
| `web_fetch`  | `images[]`、`failedResults[]`                                               | M/E/T/A/C             | 页面证据、失败诊断               |

`favicon` 的真实消费者位于 `agentWebSearch.ts` 和 `WebSearchSources.tsx`。它不是无效字段，只是不需要默认进入模型文本。

### 4.4 Todo、Goal 与历史

| Tool                               | 字段                                                                                                        | 消费者            | 归属和约束                                           |
| ---------------------------------- | ----------------------------------------------------------------------------------------------------------- | ----------------- | ---------------------------------------------------- |
| `todo_update`                      | `revision`、`updatedAt`                                                                                     | M/E/R/T/A/C       | UI、审计、恢复                                       |
| `todo_update`                      | `items[].id/title/status/note/createdAt/updatedAt`                                                          | M/E/R/T/A/C       | 行动、UI、恢复；R 生成 Todo context 和 `TodoUpdated` |
| Goal 原始结果                      | `goal.objective/status`、`created`、`updated`                                                               | M，当前也进入 E/C | 行动、恢复                                           |
| Goal Trace                         | `goalPresent`、`created`、`updated`、`status`、`goalStatePersisted`                                         | T/E               | 审计；不进入 Exact Archive                           |
| `conversation_history` Model       | `view`、turn/result/timeline/record、`content`、`navigation.*`、`open`、范围、hash、完整性标记、instruction | M/C               | 行动、正文、恢复                                     |
| `conversation_history` Trace/Event | query/open ref、读取范围、hash、返回数量、状态                                                              | T/E               | 审计；不重复保存取回正文，不再次归档                 |

Todo 的完整状态不能从 E/R 删除。以后可只收敛 M 为确认回执，因为 Extension 会注入权威 Todo snapshot；这属于下一轮行为变更。

### 4.5 文件、Patch 与命令

| Tool                   | 字段                                                                                                                                                                                                               | 消费者                | 归属和约束                                     |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------- | ---------------------------------------------- |
| `apply_patch`          | `status`、`operation`、`filePath`、`appliedFilePaths[]`                                                                                                                                                            | M/E/T/A/C             | 行动、UI、审计、恢复                           |
| `apply_patch`          | `gitDiff.patch/truncated`                                                                                                                                                                                          | M/E/T/A/C             | 行动、UI、正文                                 |
| `apply_patch`          | `gitDiffError`、`error`、`message`                                                                                                                                                                                 | M/E/T/A/C             | 冲突诊断、恢复                                 |
| `write_file` 草稿      | `draft.draftId/conversationId/projectId/filePath/mode/status/baseRevision/additions/deletions/lineCount/byteCount/chunkCount/nextChunkIndex/statsFinal/summary/createdAt/updatedAt`                                | M/E/R/T/A/C           | 行动、UI、审计、恢复；`draftId` 是权威后续路由 |
| `write_file` 草稿      | `tail`                                                                                                                                                                                                             | M/T/A/C               | 私有草稿正文；事件明确删除                     |
| `write_file` 草稿      | `maxDraftBytes`、`transactionState`、`requiresFinishBeforeResponse`、`nextAction`                                                                                                                                  | M/T/A/C，当前也进入 E | 行动、恢复                                     |
| `write_file` Host 终态 | `status`、`draftId`、`mode`、`filePath`、统计、`revision`、`error`、`message`                                                                                                                                      | M/E/T/A/C             | 权威写入回执                                   |
| `run_command`          | `command`、`cwd`                                                                                                                                                                                                   | M/E/T/A/C             | 行动、审计；与 ToolCall 重复，是 M 精简候选    |
| `run_command`          | `exitCode`、`stdout`、`stderr`、`timedOut`、`cancelled`、截断和 `error`                                                                                                                                            | M/E/T/A/C             | 行动、UI、审计、正文                           |
| `run_command`          | `durationMs`                                                                                                                                                                                                       | E/T/A/C，当前也进入 M | UI、审计                                       |
| `run_command`          | `policyEvaluation.*`                                                                                                                                                                                               | T/A/C，当前也进入 M/E | 安全审计、恢复                                 |
| `run_command`          | `artifactObservation.schemaVersion/status/coverage.*`、`expectedOutputs[]`、`changes[].kind/artifactKind/path/scope/previousPath/previousScope/before/after.*`、`changesTruncated`、`changesOmitted`、`warnings[]` | M/E/T/A/C             | 行动、UI、审计、恢复                           |
| `run_command`          | `inputFiles[].mountPath/sourceKind/sizeBytes/sha256`、`runtime.*`                                                                                                                                                  | M/E/T/A/C             | 审计、恢复；冻结输入和托管 runtime 证据        |
| Host 审计失败包装      | `type/code/recovery/phase`、执行与副作用不确定性、`auditError`、`reconciliationError`、`execution.*`                                                                                                               | M/E/T/A/C             | 行动、审计、恢复；防止有副作用后盲目重试       |

Renderer 使用 `artifactObservation` 构建 Office Artifact 卡片；失败 observation 仍保留作审计，但不能变成成功卡片。

### 4.6 Office

三个 Office Tool 共用 `OfficeExecutionResult`：

| 字段组                                                                                                           | 消费者                | 归属和约束                  |
| ---------------------------------------------------------------------------------------------------------------- | --------------------- | --------------------------- |
| `providerId`、`engineRevision`、`documentKind`、`operation`                                                      | M/E/T/A/C             | 行动、审计、恢复、幂等      |
| `argv`、`cwd`                                                                                                    | T/A/C，当前也进入 M/E | 审计、恢复；M 精简候选      |
| `exitCode`、`stdout`、`stderr`、超时/取消/耗时/截断/错误字段                                                     | M/E/T/A/C             | 行动、UI、审计、正文        |
| `outputs[].role/kind/mimeType/source/readPath/scope/readableByAgent/sizeBytes/sha256/width/height/pageSelection` | M/E/T/A/C             | 后续读取、UI Artifact、恢复 |
| 审计失败包装 `execution.*` 和不确定性字段                                                                        | M/E/T/A/C             | 行动、审计、恢复            |

`skillOfficeActivity.ts` 同时消费 Office 输出和 Command Artifact Observation，E 投影必须兼容。

### 4.7 Skill 资源和脚本

| Tool                          | 字段                                                                                                                   | 消费者      | 归属和约束                                                                                                                |
| ----------------------------- | ---------------------------------------------------------------------------------------------------------------------- | ----------- | ------------------------------------------------------------------------------------------------------------------------- |
| `skills_activate`             | `schemaVersion`、`status`、`activatedBy`、`activationRevision`、`reason`、`skill.id/name/revision/source/hasResources` | M/E/R/T/A/C | 行动、UI、恢复；R 用 `status` 和 `skill.id` 提交待激活 context、更新 ToolSet，并另发不含 instructions 的 `SkillActivated` |
| `skills_list_resources`       | `rootUri`、`resources[].uri/path/kind/byteLength/contentDigest`、`truncated`、`nextAfterPath`                          | M/E/T/A/C   | 行动、审计、恢复                                                                                                          |
| `skills_read_resource`        | `uri`、byte 范围、长度、`truncated`、`nextStartByte`                                                                   | M/E/T/A/C   | 行动、分页、恢复                                                                                                          |
| `skills_read_resource`        | `content`                                                                                                              | M/A/C       | 行动、正文；T/E 用 `contentOmittedFromHistory`                                                                            |
| `skills_materialize_resource` | `status/sourceUri/sourcePrefix/destination/sourceRevision/fileCount/byteCount/planDigest/error/message`                | M/E/T/A/C   | 行动、UI、审计、幂等                                                                                                      |
| `skills_preflight_script`     | script/skill/resource 身份、`ready`、`resourceDigest`、`preflight.*`                                                   | M/E/T/A/C   | 行动、审计、恢复                                                                                                          |
| `skills_run_script`           | script/skill/revision/digest、`preflight.*`、进程结果和截断/错误字段                                                   | M/E/T/A/C   | 行动、UI、审计、正文、恢复                                                                                                |

### 4.8 图片生成

| 字段                                                                                                                                                     | 消费者                | 归属和约束                       |
| -------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------- | -------------------------------- |
| `schemaVersion`、`status`、`operation`                                                                                                                   | M/E/T/A/C             | 行动、UI、审计、恢复             |
| `artifact.artifactId/uri/kind/format/mimeType/width/height/sizeBytes/sha256`                                                                             | M/E/T/A/C             | 行动、UI、审计、恢复             |
| `audit.executionId/requestFingerprint/providerProfileId/adapterId/profileRevision/modelId/providerRequestId/httpStatus/createdAt/completedAt/durationMs` | E/T/A/C，当前也进入 M | UI、审计、恢复、幂等             |
| `failure.code/phase/message/recovery/retryable` 与三个不确定性布尔值                                                                                     | M/E/T/A/C             | 行动、UI、审计、恢复             |
| `savedPath`、`visualInputStatus`                                                                                                                         | M/T/A/C               | 当前 Run 投递状态；E 明确去除    |
| `image.mimeType/dataBase64`                                                                                                                              | 原始 Runtime          | 原生多模态输入；所有文本投影去除 |
| `binaryOmittedFromHistory`                                                                                                                               | T/A/C                 | 二进制历史省略审计               |

图片 audit 被 Core Server action audit、启动恢复和 Renderer 图片活动共同消费，不能从原始/持久结果删除。

## 5. 下一轮精简候选

本轮不执行下列改动。每项均只建议收敛 **M 投影**，保留原始结果及其他消费者：

| 优先级 | Tool                | 建议                                                                                        |
| ------ | ------------------- | ------------------------------------------------------------------------------------------- |
| P0     | `web_search`        | M 保留 query/answer/title/url/content/date/truncated；E 完整保留 favicon/score/responseTime |
| P0     | `web_fetch`         | M 保留 canonical URL/content/truncated/有效失败摘要；E 保留 favicon                         |
| P0     | `attachments_list*` | M 保留 name/kind/mime/size/readPath；E 保留 id/messageId/name                               |
| P0     | `workspace_map`     | M 只保留 `tree[]` 或 `treeText` 一种表达                                                    |
| P0     | `todo_update`       | M 返回 revision/accepted；R/E 保留完整 `AgentTodoState`                                     |
| P0     | `run_command`       | M 保留进程终态、输出、截断、错误和简明 Artifact 变更；T/A/C 保留 policy 与完整 observation  |
| P0     | `image_generation`  | M 保留终态、artifact、失败恢复和 visual delivery；E/T/A/C 保留 audit                        |
| P1     | `write_file`        | M 按 phase 返回最小下一步回执；E/R 保留 draft snapshot，C/T/A 保留身份和正文                |
| P1     | Office              | M 成功时保留结果/输出引用/验证，失败时保留必要诊断；审计和恢复字段不动                      |
| P1     | 文档读取            | M 保留 path、分页/截断、正文和最小结构；A 保留完整正文与元数据                              |
| P1     | `skills_run_script` | M 成功时收敛重复 preflight；C/T/A 保留 digest/preflight                                     |

明确不是删除候选：

- `callId`、审批 action id、draft id、attachment id、revision、digest、hash；
- 截断标记、原始长度、cursor 和所有 `next*`；
- `artifactObservation`、图片 `audit`、审批不确定性和 effects-may-have-occurred；
- Exact Archive 的安全清洗后正文；
- Renderer 已消费的 favicon、Todo、附件定位、草稿统计和 Artifact 字段。

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

## 7. 结论

当前系统不缺投影架构；问题是多数 Tool 尚未声明专属投影，默认 clone 把 UI、审计、恢复和模型行动信息混在一起。下一轮应在统一预算下逐 Tool 实现 Model Projection，而 E/R/T/A/C 依据本矩阵独立保持。
