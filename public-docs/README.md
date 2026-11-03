---
title: Captain Who 文档
description: 从第一次使用到进阶理解，学习如何安全、有效地使用 Captain Who。
status: current
audience: public
owner: product-docs
last_verified: 2026-11-03
---

# Captain Who 文档

Captain Who 是一款本地优先的桌面 AI 工作助手。它可以在你授权的范围内理解项目、处理文件、调用工具，并把复杂任务拆给多个 Agent 协作完成。

已发布的 [`1.0.6` 安装包](releases/release-notes/1-0-6.md)适用于 macOS 12 及以上的 Apple Silicon 设备。请从 [Captain Who 官网](https://captainwhoagent.com/) 或项目的 [GitHub Release](https://github.com/Tiga001/Captain_Who/releases/tag/v1.0.6) 获取 DMG，安装前按[下载与验证](releases/download-and-verification.md)核对文件。

Captain Who 已按 Apache License 2.0 开源，[开发仓库](https://github.com/Tiga001/Captain_Who)会持续更新。仓库中的开发版本可能比官网安装包更新；想直接使用软件，请从官网下载安装包。当前仅发布 macOS Apple Silicon 版本，Windows 暂无安装包。许可证与第三方声明见[法律与第三方声明](legal/README.md)。

## 选择一条阅读路线

### 第一次使用

从[用户指南](user/README.md)开始，按顺序完成下载、安装、登录、模型配置和第一个任务。希望直接动手时，可以进入[快速开始](user/getting-started/README.md)。

### 查找具体功能

前往[能力指南](user/capabilities/README.md)，了解工具、Skill、MCP、Multi-Agent、[组织](user/capabilities/organizations.md)、浏览器自动化、Scheduled Automation 和系统通知；遇到问题时查看[自助排查](support/README.md)。

### 理解 Agent 技术

从[学习中心](user/learn/README.md)开始，用非专业化的语言理解 Agent Loop、工具调用、上下文管理和多 Agent 协作。

## 文档分区

- [用户指南](user/README.md)：上手、日常操作、能力说明、教程和技术科普。
- [扩展与集成](integrations/README.md)：创建 Skill、连接 MCP Server 和配置模型服务。
- [版本与升级](releases/README.md)：官方下载、发行说明、平台状态、校验与升级方法。
- [自助排查](support/README.md)：常见问题、诊断方法和已知问题。
- [安全中心](security/README.md)：权限、数据边界和安全使用建议。
- [法律与第三方声明](legal/README.md)：隐私政策、使用条款及随产品分发的第三方软件声明。

## 阅读说明

[发行说明](releases/release-notes/README.md)描述已经发布的安装包；[当前开发进展](releases/development-status.md)介绍标签之后的改动。例如，11 月新增的「预测下一条用户消息」不在 9 月 23 日的 `1.0.6` 安装包里，即使当前源码显示的版本号仍是 `1.0.6`。使用第三方模型、搜索或 MCP 服务时，还需遵守相应服务商的条款、计费方式和数据政策。
