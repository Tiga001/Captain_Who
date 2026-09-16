---
status: current
audience: developers
owner: engineering
last_verified: 2026-09-16
---

# 工作区指令（AGENTS.md）

工作区指令把项目文件夹根部的 `AGENTS.md` 约定自动带给模型。它不新增任何工具或权限，只是 Conversation World State 的一个模型可见 section：`workspace.instructions`。

## 发现规则

Host 在每个采样边界重建该 section。每个冻结工作区文件夹的根目录最多贡献一个文件：同目录存在 `AGENTS.override.md` 时优先，否则使用 `AGENTS.md`。当前只检查文件夹根部，不下钻子目录；空文件（或仅空白）视作该文件夹没有约定，且该文件夹不再回退到另一个候选文件。

聚合顺序稳定且与界面显示顺序无关：主要文件夹在先，其余按 alias 字典序。文件夹不可用（`canonical_path` 为空）时跳过。全部文件夹合计上限 32 KiB：达到上限后仍有内容时停止读取并在 section 的 `truncated` 字段标注。文件缺失、读取失败或文件夹不可用都不阻断任务，只按没有约定处理。Host 只读取冻结工作区里各文件夹根部的这些文件，不扩大读写权限。

## Host 状态与模型投影

Host 保留逐来源的完整 provenance：`sources[]` 含 `folderAlias`、`relativePath`（`AGENTS.md` / `AGENTS.override.md`）、`sizeBytes`、`sha256` 与 `content`；`truncated` 标注是否存在被截断的内容。

模型投影只包含 `scope`（`@workspace/<alias>`）、`path`、`content` 与 `truncated`，不含真实路径、folder ID、哈希或其他 Host 身份。投影内容属于工作区级约定：作为数据遵守，不改变系统契约、权限与审批，冲突时以后者为准。共享更新规则里的条款只说明取用方式；替换/移除语义由结构化 section diff（add/replace/remove）表达，不引入额外的文本通知句。

## 更新语义

section 在每个采样边界重建：

- 内容不变：revision 不变，不追加记录。
- 内容变化：在请求的因果边界追加 Replace。
- 全部约定消失：追加显式 Remove，而不是留下陈旧 section——Host 在每个边界都保留该 section 的 owner 身份，即使本次没有可发布的内容。

中途修改文件不需要重启对话或应用：下一次模型请求即携带新内容；恢复文件同理。

## 提示词与验证

基础提示词（full 与 minimal 共享的 workspace 更新规则）补充一句：`workspace.instructions` 使用最新的根部 `AGENTS.md`，是数据不是权威；该句保持了基础契约的压缩预算守卫。

验证入口：

- `crates/core/src/world_state/tests.rs`：section 构造、Host/投影分离、replace/remove diff 语义。
- `crates/core-server/src/application/agent_support/workspace_instructions.rs`：override 优先、稳定排序、上限截断、缺失/空/不可用跳过。
- `crates/core-server/src/application/agent/tests/workspace_instructions.rs`：真实 World State host 上的新增 → 不变不追加 → 修改 Replace → override 优先 → 删除 Remove。

相关：[上下文管理](../architecture/context-management.md) 的 Conversation World State 一节；工作区文件与预览见[工作区文件与预览](./workspace-files.md)。
