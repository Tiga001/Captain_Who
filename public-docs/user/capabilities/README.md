---
title: 能力指南
description: 了解 Tool、Skill、MCP、Multi-Agent、Scheduled Automation、浏览器与 Office 产物能力。
status: current
audience: user
owner: product-docs
last_verified: 2026-08-23
---

# 能力指南

MyCopilot 的能力由几层组成。先区分它们，能避免把“有说明”“有连接”和“已授权执行”混为一谈。

| 能力                                          | 直观理解                         | 你主要做什么                     |
| --------------------------------------------- | -------------------------------- | -------------------------------- |
| [Tool](tools.md)                              | Agent 的手和眼睛                 | 给出目标，核对工具活动和审批     |
| [Skill](skills.md)                            | 一套任务教材和配套资源           | 选择、安装、启用或更新 Skill     |
| [MCP](mcp.md)                                 | 连接外部工具的通用插座           | 配置本地 Server，授权启动和调用  |
| [Multi-Agent](multi-agent.md)                 | 由根 Agent 管理的协作小组        | 定义分工，观察子 Agent，处理审批 |
| [Scheduled Automation](automations.md)        | 定时启动根 Agent 任务            | 设置计划、目标、权限和通知       |
| [浏览器自动化](browser-automation.md)         | Agent 控制受管网页               | 开启能力，准备页面，审批敏感动作 |
| [Office 与 Artifact](artifacts-and-office.md) | 处理文档、表格、演示、PDF 和图片 | 选择 Skill、提供输入并核对输出   |

## 能力如何组合

一个任务可能同时使用多层能力。例如“每周整理行业动态并更新表格”可以是：

1. Scheduled Automation 按周启动一个根 Agent。
2. Agent 用联网 Tool 搜索资料。
3. 激活 Spreadsheet Skill，按教学流程更新工作簿。
4. 必要时把研究工作分给多个子 Agent。
5. 文件写入仍受 Scheduled Automation 冻结权限和审批规则约束。

这些层不会互相自动授权。安装 Skill 不会开启 MCP；允许 MCP Server 自动调用不会扩大文件权限；Scheduled Automation 也不会绕过普通 Agent 的审批和安全检查。

## 不确定该选什么

- 固定内置动作：先看 [Tool](tools.md)。
- 需要特定方法、模板或 Office 工作流：用 [Skill](skills.md)。
- 需要连接第三方本地工具：用 [MCP](mcp.md)。
- 子任务可以独立并行：用 [Multi-Agent](multi-agent.md)。
- 同一任务需要周期重复：用 [Scheduled Automation](automations.md)。

进一步对比见[选择合适的能力](../best-practices/choosing-capabilities.md)。
