---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-08-31
---

# FileChange 子系统

本文定义 Agent 通过专用结构化 Tool 修改本地 UTF-8 文本文件时的执行契约：`apply_patch`、Direct/Staged 两种模式、`read_file` Observation、审批、Run grant、持久审计、恢复与历史 Diff。命令、Office/Builder 等其他副作用入口使用各自的权限与审计边界；权限枚举和通用审批状态机见[Tool 体系、权限与审批](./tools-permissions-and-approvals.md)，数据库总览见[SQLite 存储与数据生命周期](../architecture/storage-and-data-lifecycle.md)。

## 1. 边界与不变量

- `apply_patch` 是模型可见的唯一专用、结构化文本文件修改 Tool；当前 FileChange 契约不保留第二套文本写入 Tool 或并行的文件 action 类型。`run_command`、Office/Builder 仍可能产生文件副作用，但不受 FileChange Observation/audit 契约保护，必须走各自的策略、审批与审计边界。
- 领域对象使用 FileChange schema v1；Direct 私有 binding 和 Observation checkpoint 当前均为 v2，Run grant 为 v1。版本域互相独立。
- 模型只提交严格的 `{"request": {...}}`。每个 action 是 closed `oneOf` 分支，缺字段、字段组合错误或未知字段都在副作用前拒绝。
- Direct 与 Staged 最终都生成同一个 `AgentProposedAction::FileChange`、冻结 proposal/binding、走同一审批与 commit 路径。
- Renderer Diff、Event、Trace、Archive 和模型拿到的 successor Observation 都不是执行授权；只有 Host 私有 binding、pending action/audit、checkpoint 与文件 identity 可组成 authority。

```text
read_file（update/delete）或 Host 私下证明 Missing（create）
  -> apply_patch Direct，或 Staged begin/append/edit
  -> 冻结 FileChange proposal + 私有 binding
  -> auto / 单次批准 / 本 Run 后续批准
  -> dispatch claim + effect-boundary revalidation
  -> 原子 publish + receipt/audit
  -> 模型专用 successor Observation；Renderer/Trace 使用脱权投影
```

## 2. Direct 与 Staged 协议

### Direct

`request.action=apply` 一次完成一个目标：

| operation        | 输入与前置条件                                                                  |                 当前上限 |
| ---------------- | ------------------------------------------------------------------------------- | -----------------------: |
| `create`         | 完整 `content`；不得带 `observationId`；Host 私下冻结 Missing 与父目录 identity |             32 KiB UTF-8 |
| `update` rewrite | `content` + 准确 `read_file` Observation                                        |             32 KiB UTF-8 |
| `update` edits   | 1–128 个 ordered exact edit + 准确 Observation；不做模糊匹配或自动 trim         | 目标 240,000 UTF-8 bytes |
| `delete`         | 准确 Observation；不得带 content/edit                                           |           无 Staged 变体 |

create 始终 no-clobber；update/delete 始终比较已观察 revision/identity。一次 proposal 只能改一个文件。

### Staged

Staged 用于超过 Direct inline 边界或需要逐步组装的 create/update：

1. `begin/create` 不带 Observation；`begin/update` 必须带 Observation，并选择 `modify`（从旧内容开始）或 `rewrite`（空草稿）。
2. `append` 或 `edit` 必须同时提交 Host 返回的精确 `transactionId`、`index` 和 `expectedDraftRevision`。
3. `status` 只读取，`abort` 显式收口草稿；`commit` 冻结最终 proposal 并进入 FileChange 审批。
4. Staged 不支持 delete。每个 append chunk 最多 1 MiB，每次 edit 接受 1–128 个 exact edit，整份草稿最多 4 MiB，草稿 TTL 为 7 天。

Staged 持久状态包括 `drafting`、`ready`、`waiting_approval`、`applying`、`applied`、`already_applied`、`rejected`、`conflict`、`failed`、`outcome_unknown`、`aborted`、`expired`。结果只使用 `definitely_not_executed`、`applied`、`outcome_unknown` 三类 outcome；不能把超时或崩溃统一降格成“未执行”。

## 3. 路径与 Observation authority

路径先经过 `FileChangePathPolicy`：

- 有 workspace 时，相对路径解析到规范 workspace；`write=all` 才能使用允许的外部绝对路径或系统别名。
- 拒绝 `..`、`.git/.hg/.svn`、symlink/reparse 父链或叶子、非普通文件，以及 Unix 上的 hard link。
- 拒绝 `pdf/doc/docx/ppt/pptx/xls/xlsx`；Office/PDF 不能伪装成文本 FileChange。
- proposal 冻结 canonical parent、稳定目录 identity、叶子 identity/revision；effect boundary 再复核，最终以 no-follow/no-clobber 或 compare-and-publish 原子提交。

`read_file.fileChangeTarget` 是 update/delete 的来源证明。Observation 绑定 Run、Conversation、Tool Call、canonical target、父目录 identity 和叶子状态：

- claim TTL 为 10 分钟，每个 Run 最多 1,024 个；格式错误的 proposal 不消费它，通过结构与绑定校验的 proposal 才原子 claim 一次。
- TTL 限制“何时可 claim”。Observation 一旦已冻结进完整待审批 transaction，人工审批可以晚于原 TTL。
- 同一 Provider Tool Call 批次不能并行复用一个 Observation；必须等成功 Tool Result 被模型实际观察后再继续。
- 成功 create 返回该目标的第一个公开 Observation ID；成功 update/delete 或 Staged update commit 在写后复核通过后续约输入的同一个 ID。
- 若写后发生外部竞争或续约失败，文件提交仍保持成功，但模型结果设置 `observationRefreshRequired=true` 并给出 `read_file` continuation；系统不能猜测新 authority。

## 4. 审批与 Run grant

`apply`/`commit` 先准备 typed FileChange：冻结 operation/path、base/target、proposal/diff digest、Run/Conversation/Project、permission/toolset/provider revision 和私有执行材料。用户看到的 Diff 只作展示，不会成为执行 payload。`patch=auto_approve` 也必须完成 prepare、持久 pending/audit、checkpoint、dispatch claim 和 effect-boundary revalidation；它只省去点击。

协议支持两种批准范围：

- `SingleAction`：只批准当前精确 FileChange。
- `RemainingApplyPatchInRun`：当前 create/update 成功后，为本 Run 后续合规的 `apply_patch` create/update 建立持久 grant；delete 永远不受该 grant 授权。

Run grant 的安全边界：

1. 用户选择范围时先写 `pending` intent，它没有执行权。
2. 只有授予它的 FileChange 以匹配的 applied result digest 结算后，intent 才变为 `active`；每个 Run 最多一个 active grant。
3. grant 冻结 Run/Conversation/Project、原 write ceiling、permission/toolset/provider revision、`apply-patch-file-change-v1` contract revision、授予 action/receipt，以及精确 workspace root 或 external parent 与稳定目录 identity。
4. 后续 proposal 和真正 effect boundary 都重新加载并验证 durable grant。缺失、损坏、作用域变化或 revision 不符时结果为 definitely-not-executed，并重新请求批准。
5. checkpoint 中的 grant ref 只是索引，不单独构成 authority。Run 终态、取消、恢复身份不匹配或 grant owner 丢失时必须 revoke。

## 5. Commit、审计与恢复

FileChange 在写入前再次读取绑定状态；create 使用 no-clobber publish，update 使用 compare-and-publish，delete 使用可恢复 journal。成功 receipt 必须与冻结 transaction 的 operation/path/base/target/proposal digest 完全匹配。

持久 action audit 保存 Host 私有 `action_json`、`file_change_result_json` 和 terminal Tool Result，用于重复审批返回同一结果、启动恢复和历史 Diff。`executing` audit、`applying` Staged transaction、delete journal 及 pending/active Run grant 在启动时对账：

- 能由目标 digest/receipt 证明已完成的动作收敛为 `applied`/`already_applied`；
- 明确尚未执行的动作安全失败；
- 已跨副作用边界但无法证明结果的动作保持 `outcome_unknown`，绝不盲重放；
- Run guard 只把可变 `drafting/ready` 草稿收口为 failed/aborted；`waiting_approval` 属于 pending action，`applying` 留给专用 reconciliation。

## 6. 投影与敏感边界

FileChange 对不同消费者使用不同投影：

| 消费者                    | 可见内容                                                              | 明确排除                                                                     |
| ------------------------- | --------------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| Model                     | canonical result；成功后附加 successor `fileChangeTarget`，或要求重读 | Host canonical path、目录 identity、私有 binding                             |
| Renderer/Event            | proposal、状态、统计、分页 Diff/进度                                  | Observation authority、执行 binding、完整正文                                |
| Durable Trace/Archive     | body-free call operation、content/edit digest、无权结果               | `observationId`、`fileChangeTarget`、`observationRefreshRequired` 与调用正文 |
| Private checkpoint        | 恢复所需的冻结状态，以及模型已观察的 successor                        | 不向 Renderer/RPC 投影                                                       |
| Action audit/Staged store | 精确审批、commit、恢复与历史 Diff 材料                                | 不作为公共 Tool Result 返回                                                  |

`apply_patch` 的 durable call Trace 不保存完整 content、old/new text 或 Observation ID，而保存安全 operation 与 digest。successor Observation 只进入模型投影和私有 checkpoint；Event、Trace、Archive 以及持久公共结果都会剥离这些字段。

## 7. 持久化、历史 Diff 与分叉

canonical schema v33 中的 FileChange 数据分为：

- `agent_file_changes`、`agent_file_change_chunks`、`agent_file_change_operations`：仅保存 Staged create/update 草稿、mutation receipt 与可见历史；Direct 不在这里伪造草稿。
- `agent_file_change_run_grants`：Run-scoped runtime authority，不是聊天历史。
- `agent_pending_actions` 与 `agent_action_audit.file_change_result_json/action_json`：Direct/Staged commit 的审批、receipt、恢复和历史 Diff authority。

读取接口：

- `agent.readFileChange`：读取 live/persisted Staged 内容页；
- `agent.getFileChangeDiff`：读取 Staged 当前 Diff 页；
- `agent.getFileChangeHistoryDiff`：用 exact `conversationId + assistantMessageId + runId + toolCallId` 读取 terminal action audit。Direct 使用冻结 inline Diff，Staged 从 binding 重建，不依赖 Trace 中的正文。

三类分页默认 50,000 字符，`maxChars` 被夹在 1,000–100,000。当前 Conversation 的用户写权限或“精确根 Conversation 观察精确子 Conversation”才能读取；调用方不能用猜测 ID 探测其他会话。

Conversation fork 会复制并重映射可见的 Staged tables，以及已完成/失败/取消/拒绝的 terminal FileChange action audit；pending/executing action 不复制。`agent_file_change_run_grants` 是 runtime-only，分叉绝不继承。历史卡片使用 lazy-loaded、分页的 split Diff review；Git review 中的路径展示保持 project-relative，但展示路径不参与 FileChange 授权。

## 8. 当前边界快照

| 边界                                    |                           当前值 |
| --------------------------------------- | -------------------------------: |
| FileChange domain / Run grant schema    |                          v1 / v1 |
| Direct binding / Observation checkpoint |                          v2 / v2 |
| Direct complete content                 |                     32 KiB UTF-8 |
| Direct structured edits                 |        1–128；目标 240,000 bytes |
| Staged chunk / transaction              |                    1 MiB / 4 MiB |
| Staged draft TTL                        |                             7 天 |
| Observation claim TTL / per Run         |                  10 分钟 / 1,024 |
| content/diff/history page               | 默认 50,000 chars；1,000–100,000 |

## 9. 代码真源

- 领域类型与 policy：`crates/core/src/file_change/`
- Tool wire、投影与 successor：`crates/core/src/tools/apply_patch.rs`
- Staged：`crates/core/src/tools/file_change_staged.rs`、`file_change_stream.rs`
- Trace 安全投影：`crates/core/src/file_change_support.rs`、`conversation_trace_projection.rs`
- checkpoint：`crates/core/src/runtime/checkpoint.rs`
- 审批、执行与恢复：`crates/core-server/src/application/agent/approval.rs`、`action_execution/file_change_authorization.rs`、`pending_action_store.rs`
- 存储：`crates/core/src/storage/file_change_repository.rs`、`file_change_run_grant_repository.rs`、`agent_action_audit_repository.rs`
- Host API：`crates/core-server/src/transport/agent_rpc.rs`、`packages/protocol/src/agent.ts`

## 10. 测试

重点覆盖：

- `crates/core/src/file_change/tests.rs`、`file_change/observation.rs`、`file_change/run_grant.rs`
- `crates/core/src/tools/apply_patch.rs`、`file_change_round6_acceptance.rs`
- `crates/core-server/src/application/agent/tests/file_change_permissions.rs`
- `crates/core-server/src/application/agent/tests/file_change_source_boundary.rs`
- `crates/core-server/src/application/agent/tests/staged_file_change_execution.rs`
- `crates/core/src/storage/conversation_fork_repository/tests.rs`

## 11. 当前限制

- 只支持单目标 UTF-8 文本；没有多文件原子 transaction，Office/PDF 使用各自受管流程。
- Staged 不支持 delete；Direct 大正文必须转为 Staged。
- Observation 和 Run grant 都是 Run-bound；不能跨 Run、跨 Conversation 或靠复制 opaque ID 使用。
- 文件系统与 SQLite 不能共享一个 ACID transaction；正确性依赖 bound I/O、journal、receipt 和启动 reconciliation。
- 历史 Diff 是有界展示接口，不是执行 payload，也不是任意时刻文件系统快照。

## 12. 变更检查表

- [ ] Tool schema 是否保持 closed `request.oneOf`，并同步 Rust/TypeScript protocol fixture？
- [ ] 新 operation/mode 是否定义 Observation、path identity、atomic publish 和冲突语义？
- [ ] proposal、pending action、audit、checkpoint 与 receipt 是否绑定同一 owner/revision/digest？
- [ ] AutoApprove 与 Run grant 是否仍走完整 prepare/claim/revalidate 路径？
- [ ] Event/Trace/Archive 是否继续去除正文和 Observation authority？
- [ ] crash window 是否可判定 definitely-not-executed、applied 或 outcome-unknown，且不盲重放？
- [ ] fork/delete/rewrite 是否明确处理 Staged history、terminal audit 和 runtime-only grant？
- [ ] 修改限额后是否同步消费者矩阵、上限文档、Renderer 分页与边界测试？
