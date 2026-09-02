---
title: 用户指南
description: 从第一次启动到使用 Agent、Skill、MCP、多智能体和定时任务的完整学习入口。
status: current
audience: user
owner: product-docs
last_verified: 2026-08-31
---

# 用户指南

这套文档面向第一次接触 Captain Who 的用户。你不需要先理解大模型或编程术语；可以先完成一个任务，再按需了解 Agent 为什么能够读取文件、调用工具和继续多步工作。

Captain Who 是一款**本地优先的桌面 AI 工作助手**：应用、项目授权、对话记录和大部分运行状态在本机管理；模型请求、联网搜索和你主动连接的第三方服务仍可能把必要数据发送到对应服务商。“本地优先”不等于“完全离线”。

## 推荐学习路线

### 10 分钟快速上手

1. 阅读[产品概览](getting-started/product-overview.md)，确认它是否适合你的工作。
2. 按[安装与首次启动](getting-started/installation.md)取得当前可用版本。
3. 跟随[完成第一个任务](getting-started/first-task.md)配置模型并发起对话。
4. 如果要处理一个文件夹或代码仓库，继续[使用第一个项目](getting-started/first-project.md)。

### 按功能查找

- [日常使用](everyday-use/README.md)：对话、文件、Git、联网、模型、权限和历史。
- [能力指南](capabilities/README.md)：Tool、Skill、MCP、Multi-Agent、Scheduled Automation、系统通知、浏览器自动化和 Office 产物。
- [任务教程](tutorials/README.md)：围绕真实目标完成一整套操作。
- [最佳实践](best-practices/README.md)：更清晰地描述任务、管理大型项目并安全使用 Agent。
- [参考手册](reference/README.md)：快速查询设置、状态、快捷键、能力边界和术语。
- [常见问题](../support/faq.md)：从现象快速找到解决方法。

### 从使用走向理解

从[聊天机器人到 Agent](learn/from-chatbot-to-agent.md)开始，再阅读 [Agent Loop](learn/agent-loop.md)、[上下文管理](learn/context-management.md)和[多智能体协作原理](learn/multi-agent-principles.md)。这些文章强调直觉和例子，不要求阅读代码。

## 使用时记住三件事

1. **Agent 能做什么，取决于当前模型、项目、权限、Skill 和 MCP 配置。**界面里出现某项能力，不代表当前任务已经获得授权。
2. **审批前先核对对象和影响。**尤其是命令、文件修改、浏览器敏感操作、Skill 安装和外部 MCP 调用。
3. **外部操作不能保证回滚。**停止任务可以阻止后续步骤，但已经写入的文件、已经发送的请求或已经触发的第三方操作可能仍然存在。

## 文档边界

这里写的是用户可以观察和操作的行为，不公开内部协议、数据库结构或维护流程。开发者文档与公开用户文档分开维护；如果界面与本文不一致，请先确认应用版本，并以该版本实际显示的可用项为准。
