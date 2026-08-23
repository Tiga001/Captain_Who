---
title: Skill
description: 选择、安装和管理 Skill，并理解 Skill 与工具、权限和脚本的关系。
status: current
audience: user
owner: product-docs
last_verified: 2026-08-23
---

# Skill

Skill 是一套面向 Agent 的任务说明包，可以同时包含教学指令、参考资料、模板、资源和脚本。它告诉 Agent“这类任务应该怎么做”，但不会因为被安装或选中就自动获得文件、命令、网络或 MCP 权限。

## 三种来源

| 来源   | 说明                                            | 可见范围   |
| ------ | ----------------------------------------------- | ---------- |
| 内置   | 随应用提供，例如文档、PDF、表格、演示、图片生成 | 所有任务   |
| 已安装 | 由用户从 GitHub 或本地文件夹安装                | 当前用户   |
| 工作区 | 项目内 `.agents/skills/<名称>/SKILL.md`         | 仅当前项目 |

工作区 Skill 来自项目可编辑内容，默认按不受信任的指令看待。已安装 Skill 经过用户确认，但“用户已安装”仍不等于其每个脚本都安全。

## 在任务中选择 Skill

1. 新建或打开一个对话。
2. 点击输入框的“+”。
3. 选择“技能”。
4. 搜索并勾选需要的 Skill。
5. 核对来源标识，再发送任务。

当前一次运行最多选择或激活 8 个 Skill。选择越多不一定越好；无关指令会占用上下文并增加冲突。通常只选最接近目标的一到两个。

如果已选 Skill 在发送前发生更新，界面会要求“使用最新版本”；如果它被移除或离开了所属项目，需要移除失效选择后再发送。

Agent 也可以从当前可用目录中发现并激活匹配 Skill。激活只在本次运行有效，不会让 Skill 永久驻留在对话中。

## 安装第三方 Skill

1. 打开“设置 → 技能”。
2. 点击“安装技能”。
3. 选择“从 GitHub 安装”或“从本地文件夹安装”。
4. GitHub 支持公开仓库首页、仓库内目录或精确 `SKILL.md` 链接；一个来源含多个 Skill 时，选择其中一个。
5. 等待 MyCopilot 检查包内容。
6. 阅读来源、文件数量、脚本、变化和安全警告，再确认安装。

安装预览有有效期；过期后需要重新检查。安装完成后，可以在技能列表中启停、更新或卸载。

卸载只移除 MyCopilot 管理的本地 Skill 包，不会修改原 GitHub 仓库或你最初选择的源目录。

## Workspace Skill 的最小结构

一个最小工作区 Skill 形如：

```text
.agents/
└── skills/
    └── my-skill/
        └── SKILL.md
```

入口文件必须精确命名为 `SKILL.md`。Skill 还可以带 `references/`、`assets/`、`templates/`、`scripts/` 等资源。创建完整 Skill 请参考[创建 Skill 教程](../tutorials/create-a-skill.md)，不要直接修改随应用安装的内置 Skill。

## Skill 脚本的特殊边界

当前 Skill Script 只支持包内 Python 脚本，并依赖本机可验证的 Python 3。运行脚本要求完全读写范围和 Full Access 前置条件，而且每次执行都需要明确审批；缺少 Python 依赖时只会报告，不会自动安装。

这意味着“脚本来自一个 Skill”不是自动执行理由。审批时仍应核对 Skill、脚本、参数、依赖和目标项目。

## 内置 Office Skill

创建或编辑 Word、Excel 和 PowerPoint 依赖相应内置 Skill。底层 Office 工具主要负责检查、验证和渲染；真正的新建与编辑由 Skill 提供的受管 Builder/Editor 完成。

具体步骤见[Office、PDF、表格、演示和图片](artifacts-and-office.md)。

## 当前限制

- 第三方安装来源主要是公开 GitHub 和已授权本地文件夹，没有通用 Skill 商店。
- 激活只持续一次运行，不能设置为某个对话永久启用。
- 超大 Skill 目录会被裁剪并显示诊断。
- Skill 不是操作系统沙箱，项目目录也可能被其他程序同时修改。

## 相关内容

- [Skill 与 MCP 的区别](../learn/skills-and-mcp.md)
- [创建和使用 Skill](../tutorials/create-a-skill.md)
- [权限与审批](../everyday-use/permissions-and-approvals.md)
