---
title: 创建一个 Workspace Skill
description: 使用内置 Skill Creator，把项目中的稳定做法沉淀为可复用 Skill。
status: current
audience: user
owner: product-docs
last_verified: 2026-08-23
---

# 创建一个 Workspace Skill

## 目标

在当前项目中创建一个可发现、可测试的 Workspace Skill，让后续任务能够复用团队约定，而不是每次重新解释。

## 准备

- 使用已添加的项目；Workspace Skill 保存在该项目的 `.agents/skills/` 下。
- 明确一个稳定且重复的流程，例如“按团队格式撰写变更说明”，不要把一次性需求做成 Skill。
- 准备 2～3 个真实示例和成功标准。
- 确认内置 `skill-creator` Skill 已启用并能在输入框的 Skill 选择器中找到。

## 步骤

### 1. 在对话中选择 Skill Creator

在项目对话输入框点击“添加上下文（+）”，再选择“技能”，搜索并选择 `skill-creator`。选择只是把该 Skill 加入当前任务；Skill 本身不会自动获得文件、命令或网络权限。

### 2. 描述要沉淀的行为

发送一条具体任务：

```text
请使用 skill-creator 为本项目创建一个 Workspace Skill。

名称：release-note-helper
用途：根据实际 Git diff 撰写中文变更说明。
触发场景：用户要求生成发布说明或月报中的变更摘要。
要求：
- 先读取实际 diff，不根据聊天记忆臆测；
- 按“新增、修复、风险与验证”组织；
- 没有证据的测试不得写成已通过；
- 不执行 git commit、push 或发布。

请先给出 Skill 结构和两个示例，再创建文件。完成后做静态检查，并说明如何在新任务中验证。
```

Skill Creator 会根据需要创建精确大小写的 `SKILL.md`，并可能加入 `references/`、`templates/`、`assets/` 或 `scripts/`。内容应聚焦工作方法；不要把大量通用知识全部塞进入口文件。

### 3. 审查文件

批准写入前核对目标必须位于：

```text
.agents/skills/release-note-helper/
```

检查 `SKILL.md` 中的名称、描述和触发条件是否清楚，资源文件是否真的必要。Workspace Skill 来自项目内容，默认应按不受信任输入看待。

### 4. 在新对话验证

新建一个项目对话，在 Skill 选择器中刷新并选择新 Skill，然后用一个真实案例测试。当前 Run 不会热加载刚创建或修改的 Workspace Skill，因此不要在创建它的同一 Run 中声称已完成行为验证。

验证时检查三件事：是否容易被正确发现、是否按要求完成、是否在不相关任务中误触发。若需修改，回到文件修订后再次用新的 Run 验证。

## 预期结果

项目中出现一个结构合法的 `.agents/skills/<name>/SKILL.md`；新对话能发现并选择它；真实测试输出符合成功标准。Workspace Skill 只对当前项目可见，不会自动安装到全局 Skill 列表。

## 失败处理

- **找不到 Skill Creator**：前往“设置 → 技能”确认它已启用，再刷新输入框的 Skill 目录。
- **新 Skill 没出现**：必须开启新对话或新 Run，并刷新目录；确认文件名是精确的 `SKILL.md`。
- **同名冲突**：为 Skill 使用更明确的唯一名称，不要依赖模糊的同名覆盖。
- **脚本无法运行**：当前 Skill Script 仅支持 Skill 已声明的 Python 脚本，需要“完全权限”，并且每次执行都要审批；缺失依赖不会自动安装。
- **想修改已安装 Skill**：不要直接改应用受管目录。复制到 Workspace 形成可编辑版本，再审查和测试。

继续阅读[Skill 与 MCP](../learn/skills-and-mcp.md)和[编写有效任务](../best-practices/writing-effective-tasks.md)。
