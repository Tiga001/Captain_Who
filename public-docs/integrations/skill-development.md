---
title: 开发 Skill
description: 为 Captain Who 创建、测试和安装面向任务的 Skill 包。
status: current
audience: integration-developer
owner: developer-relations
last_verified: 2026-08-31
---

# 开发 Skill

Skill 是一组教 Agent 如何完成特定工作的说明，可以附带参考资料、模板、资源和 Python 脚本。它适合沉淀流程，不适合保存密码，也不会绕过用户权限。

## 创建第一个 Workspace Skill

在项目根目录创建：

```text
.agents/
└── skills/
    └── release-review/
        └── SKILL.md
```

`SKILL.md` 必须使用这个精确大小写。最小示例：

```markdown
---
name: release-review
description: 检查发布候选版本的变更、测试和已知风险。用于用户要求发布审查时。
---

# Release review

1. 读取本次变更与测试结果。
2. 区分已验证事实和待确认事项。
3. 输出阻断项、风险和建议。
```

建议使用小写、短横线分隔的目录名和 `name`。`description` 应同时说明“做什么”和“什么时候使用”，因为 Agent 会依据它决定是否激活 Skill。

## 添加资源

Skill 可以包含：

```text
release-review/
├── SKILL.md
├── references/     # 按需阅读的详细说明
├── assets/         # 图片或其他资源
├── templates/      # 可复用模板
└── scripts/        # Python 脚本
```

把必须遵守的步骤保留在 `SKILL.md`，把较长的背景材料放在引用文件中。使用相对链接，并确保大小写与实际文件一致。不要放入 Token、私钥、个人数据或本机绝对路径。

## 测试 Skill

至少准备三类测试请求：

1. **应触发**：用户明确需要该工作流。
2. **边界情况**：信息不完整、输入较大或需要审批。
3. **不应触发**：主题相近，但不需要该 Skill。

在新的任务中测试，确认 Agent 选择了正确 Skill、遵循了关键步骤，并且没有因为说明过宽而误触发。修改 Skill 后重新运行，因为一次已经开始的 Run 会继续使用它启动时冻结的版本。

## 脚本与权限

当前 Skill Script 只支持 `scripts/*.py`，并依赖电脑上工作区之外可用的 Python 3。脚本执行不是操作系统沙箱：预检和执行都需要完整的读写与命令权限。Installed/Workspace Skill 脚本每次运行都会请求明确批准；只有应用能重新证明来源、revision 和内容完整性的 Bundled Skill 脚本，才可能在用户启用“内置执行自动批准”且其他条件均满足时免去点击。依赖缺失时 Captain Who 只报告问题，不会自动安装包。

因此，能用说明或模板解决时不必添加脚本。添加脚本时应使用结构化参数、限制输入输出、说明副作用，并避免读取未授权位置。

## 安装和管理

- Workspace Skill 位于当前项目的 `.agents/skills/`，不会自动变成全局 Skill。
- 在“设置 → 技能”中可以查看内置和已安装 Skill，并启用、更新或卸载可管理的条目。
- 第三方安装当前支持 GitHub 来源和用户授权的本地目录。应用会先展示检查结果，再请求安装批准。
- 已安装包由应用管理，不应直接修改其文件。要改动它，请先在项目中制作 Workspace 副本，再测试和安装新版本。

## 当前限制

- 单次 Run 最多激活的 Skill 数量和内容都有安全预算；超大目录可能被裁剪并显示诊断。
- 其他在线 Skill Registry 尚未作为通用安装来源开放。
- Skill 激活仅对当前 Run 生效，不是跨对话永久激活。
- Skill 来源或“已安装”状态不授予额外 Tool 权限。
