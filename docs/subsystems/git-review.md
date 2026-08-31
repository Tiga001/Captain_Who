---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-31
---

# Git Review 子系统

Git Review 在右侧栏中提供仓库变更摘要、按文件差异、完整内容对照，以及有限的暂存、取消暂存和恢复操作。Git 命令、快照一致性与文件系统安全由 Rust Core 负责；Electron Main 仅转发协议；Renderer 负责按需加载、缓存和展示。整体 IPC 约束见 [IPC 与协议](../architecture/ipc-and-protocol.md)，面板生命周期见 [右侧栏平台](./right-sidebar.md)。

## 职责与边界

| 层                 | 职责                                                                                | 不负责                                 |
| ------------------ | ----------------------------------------------------------------------------------- | -------------------------------------- |
| Rust Core          | 探测仓库、生成一致性快照、执行 Git、解析受限 diff、读取完整内容、校验并执行变更操作 | UI 状态、长期保存 diff、仓库外路径操作 |
| Core Server        | 将 Git 领域方法暴露为 RPC；为 `lastTurn` 解析会话上下文                             | 重写 Git 语义                          |
| Electron Main      | 注册 Git IPC，调用 Core Server，验证 Renderer 来源                                  | 直接运行 Git 命令                      |
| Preload / Host API | 暴露类型化调用和事件边界                                                            | 暴露任意 Electron 或 Node API          |
| Renderer           | 选择范围、按需加载文件内容、缓存、搜索、统一/分栏显示、确认危险操作                 | 将缓存视为权威状态                     |

代码中的仓库状态为 `ready`、`notRepository`、`unsupported` 和 `unavailable`。只有 `ready` 状态允许读取 diff 或执行变更操作。

## 数据模型

### Review 范围

- `unstaged`：工作区相对索引的变化，允许暂存与恢复。
- `staged`：索引相对 `HEAD` 的变化，只允许取消暂存；必须先取消暂存，才能从 `unstaged` 范围恢复。
- `lastTurn`：最近一次 Agent Turn 前后记录的工作区变化，只读，并要求 `conversationId`。

文件状态包括 `modified`、`added`、`deleted`、`renamed`、`copied`、`untracked` 和 `conflicted`。公开的 `path` 使用斜杠分隔，并始终相对当前选中 project root；即使 project 是更大 Git worktree 的子目录，也不能把仓库根前缀泄漏到 UI。`previousPath` 只在旧路径同样位于该 project 内时出现。摘要返回 `repositoryId`、`snapshotId`、统计信息、文件列表及 `truncated` 标记。后续 diff、完整内容和变更请求都必须携带同一身份信息；Renderer 不应自行拼接 Git 路径或推断重命名关系。

### 响应降级

单文件 diff 的结果为 `ready`、`binary`、`tooLarge` 或 `snapshotExpired`；完整内容另有 `unsupported`。这些状态是协议的一部分，不是异常字符串。UI 应保留文件条目并展示原因，不应把二进制或超限文件静默当成空 diff。

## 关键流程

### 加载摘要与差异

1. `useGitRepositoryCapability` 根据当前项目和工作区建立能力上下文；没有有效工作区时不发起请求。
2. `useGitReview` 请求所选范围的摘要，并以项目、范围、会话和返回的快照身份作为缓存边界。
3. Renderer 最多并发加载 4 个 diff，请求队列最多 64 个；缓存最多 64 个文件或约 2400 万字符。
4. 用户请求完整文件对照时，使用独立队列：最多并发 4 个、排队 16 个，缓存最多 24 个文件或约 1200 万字符。
5. Rust Core 只接受属于当前快照的文件身份，并在限制内运行 Git、解析结果。
6. 若任一请求返回 `snapshotExpired`，Renderer 丢弃该轮衍生状态并刷新摘要，而不是把新旧快照混合展示。

Renderer 还有独立的渲染预算：diff 约 512 KiB 字符、12,000 行、1,000 个 hunk，单行最多 32 KiB；完整内容水合约 512 KiB 和 20,000 行。Rust Core 限制负责安全和资源上界，Renderer 预算负责界面响应性，两者不可互相替代。

### 快照一致性

Rust Core 快照在内存中保存，默认有效期 5 分钟；普通 Review 与 `lastTurn` 各自的快照缓存最多 32 个。快照记录 `HEAD`、索引戳和未暂存文件戳；读取 diff、内容或变更前会再次验证相关状态。Rust Core 级主要上界包括：

- Review 最多返回 500 个文件。
- 单文件 diff 最多读取约 512 KiB、20,000 行、2,000 个 hunk，单行最多 32 KiB。
- 完整内容每侧最多约 1 MiB、100,000 行，总量约 2 MiB。
- Git 命令默认超时 12 秒；标准错误输出受限。

任何上界调整都需要同步检查协议降级状态、Renderer 预算和测试，不能只提高某一层常量。

### 变更操作

协议只支持 `stage`、`unstage` 和 `restore`。请求必须携带当前 `snapshotId`；Rust Core 在执行前重新验证快照，成功后返回 `applied`，过期则返回 `snapshotExpired`。`stage` 仅接受 `unstaged`，`unstage` 仅接受 `staged`，`restore` 也仅接受 `unstaged`。

安全规则：

- 所有路径都来自快照中的文件身份，并以 `--` 后的字面 pathspec 交给 Git；不接受任意用户命令或参数。
- 重命名文件会同时处理快照记录的原路径，不能由 Renderer 猜测。
- 恢复未跟踪文件只允许删除单个受控文件，拒绝目录、仓库逃逸和不安全的符号链接目标。
- 需要外部 Git filter 的内容会被拒绝，避免执行仓库配置引入的任意程序。
- Renderer 在 `restore` 前必须给出明确确认；暂存与取消暂存仍需展示执行结果。

操作成功后应刷新摘要。不得本地修改缓存来模拟 Git 已完成，因为仓库可能同时被终端、Agent 或外部编辑器改变。

### `lastTurn`

Agent Run 会持久化 Turn 开始前与结束后的 Git 状态。`lastTurn` 通过当前会话取最近一条可用记录，并比较这两个边界；它不等同于当前工作区，也不允许变更操作。没有会话、没有记录或记录已失效时，UI 必须显示明确空态或不可用状态。

### 展示与偏好

面板支持统一/分栏 diff、换行、文本搜索、范围切换，并可通过右侧栏 Files 模块打开文件。该跳转使用 Files 的 transient preview 语义：连续查看可替换临时页，重复打开同一文件会将其稳定化；详情见[工作区文件](./workspace-files.md)。语法高亮在 Worker 中按需执行，超出预算时保持纯文本或降级视图。`loadFullFiles` 与 `showAllFileTypes` 等偏好仅存入 Renderer 的 `localStorage`；仓库元数据、文件内容和 diff 不得持久化到该位置。

Agent `FileChange` 卡片和审批对话框复用 `GitPatchRenderer`、parser 与渲染预算来显示分栏 patch，但它们不是 Git Review：FileChange 的活动 diff 绑定 transaction，历史 diff 绑定 conversation/message/Run/tool-call action audit；Git Review 绑定 repository/snapshot/scope。复用展示组件不允许复用 mutation callback、snapshot id 或缓存 key，也不使 FileChange 获得 stage/restore 权限。详见 [FileChange 子系统](./file-change.md)。

## 状态与安全不变量

1. `repositoryId + snapshotId + scope + conversationId` 共同限定一次 Review；任何一项变化都必须隔离旧请求与缓存。
2. Rust Core 是路径、仓库边界、快照有效性和 Git 参数的最终裁决者；Renderer 校验只用于体验优化。
3. 不得在 Main 或 Renderer 中新增直接的 `git` 子进程调用。
4. `snapshotExpired` 必须触发重新取摘要；不得重放带旧快照的变更操作。
5. `lastTurn` 永远只读，且不能退化为当前 `unstaged` 结果。
6. 二进制、超限、冲突和不支持状态必须保留为结构化状态。
7. 恢复操作必须经过用户确认，且只能作用于快照中的单个受控文件身份。
8. 异步结果写入 UI 前必须验证当前项目、会话、范围和快照身份。
9. 所有公开 Git path 均相对 selected project root；仓库根前缀和 project 外 `previousPath` 不进入协议。
10. Git Review 与 FileChange 可共享 renderer，但 authority、分页和 mutation 永远分离。

## 代码真源

- 协议与状态：`packages/protocol/src/git.ts`
- Rust Core 聚合入口与限制：`crates/core/src/git_review.rs`
- 快照：`crates/core/src/git_review/snapshot.rs`
- 变更安全：`crates/core/src/git_review/mutation.rs`
- Core Server RPC：`crates/core-server/src/transport/git_rpc.rs`
- Main IPC：`src/main/ipc/gitIpc.ts`
- Renderer 能力：`src/renderer/src/features/gitReview/useGitRepositoryCapability.ts`
- Renderer 编排与缓存：`src/renderer/src/features/gitReview/useGitReview.ts`
- Renderer 渲染预算：`src/renderer/src/features/gitReview/diff/gitDiffRenderBudget.ts`
- 共享 patch renderer：`src/renderer/src/features/gitReview/GitReviewDiffRenderer.tsx`
- Git Review UI：`src/renderer/src/features/gitReview/GitReviewPanel.tsx`

字段名称、枚举与限制以代码和协议为准；本文用于解释跨层约束。

## 测试

修改时至少覆盖：

- `crates/core/src/git_review/tests.rs`：仓库探测、解析、限制、快照失效和变更安全。
- `crates/core-server/src/transport/git_rpc.rs` 附近测试：RPC 映射与 `lastTurn` 会话解析。
- `src/renderer/src/features/gitReview/__tests__/`：请求身份、缓存、预算、搜索、变更确认与可用性。
- `packages/protocol/src/git.ts` 当前没有独立解析测试；新增运行时解析器时必须同时补测试。
- `src/main/ipc/gitIpc.ts` 当前没有独立测试；改变可信 sender 或转发映射时必须补 Main 层覆盖。

除自动化测试外，应在普通仓库、非仓库、含未跟踪文件、重命名、冲突、二进制文件和大文件的工作区中各做一次手工验证。

## 变更检查表

- [ ] 协议变更先更新 `packages/protocol`，Core Server、Host API 和 Renderer 无宽松兜底。
- [ ] 新范围或状态明确快照身份、是否可变更及失效行为。
- [ ] 新 Git 命令仍使用固定参数、超时、输出上界和字面 pathspec。
- [ ] 路径操作覆盖仓库逃逸、符号链接、目录、重命名与外部 filter。
- [ ] project 位于更大 worktree 子目录时，摘要/diff/完整内容与 mutation 均只使用 project-relative path，project 外旧路径不进入 `previousPath`。
- [ ] Rust Core 上界与 Renderer 加载/渲染预算一并评估。
- [ ] 切换项目、会话、范围、快照时，旧异步结果不能污染新视图。
- [ ] 危险操作有确认、结构化结果及操作后的权威刷新。
- [ ] 更新相关单元测试、集成测试和本文的限制说明。
- [ ] FileChange 复用只发生在纯展示层，没有混用 Git snapshot/cache/mutation identity。

## 当前限制

- 每次摘要最多 500 个文件；超出时通过 `truncated` 明示，不提供完整仓库级分页。
- 快照仅存在于 Rust Core 进程内，默认 5 分钟；普通 Review 与 `lastTurn` 缓存各最多 32 个，应用重启后不可恢复。
- 只提供暂存、取消暂存和恢复，不提供提交、分支、合并、rebase 或 stash 工作流。
- `lastTurn` 只读取当前会话最近一次可用 Turn，且不能用于回滚。
- 二进制、超限和需要外部 filter 的文件只提供降级状态。
- UI 偏好是本机 Renderer 范围的轻量持久化，不在设备或工作区之间同步。
