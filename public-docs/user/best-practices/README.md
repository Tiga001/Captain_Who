---
title: 最佳实践
description: 用更清楚、更安全、更易验证的方法组织 Captain Who 任务。
status: current
audience: user
owner: product-docs
last_verified: 2026-08-23
---

# 最佳实践

好的 Agent 结果通常不是来自“更长的提示词”，而是来自清晰边界、合适能力和可观察的完成证据。

- [编写有效任务](writing-effective-tasks.md)：把目标写成可执行、可验收的任务。
- [管理大型项目](managing-large-projects.md)：按阶段控制上下文、风险和变更范围。
- [选择合适的能力](choosing-capabilities.md)：判断何时使用 Tool、Skill、MCP、Multi-Agent 或 Scheduled Automation。
- [安全使用 Agent](safe-agent-usage.md)：处理权限、凭据、第三方内容和未知结果。

这些原则适用于普通对话、子 Agent 和 Scheduled Automation。定时任务的 Prompt 尤其需要独立完整，因为它可能在你不看界面时运行。
