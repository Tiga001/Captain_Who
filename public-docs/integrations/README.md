---
title: 扩展与集成
description: 了解 Captain Who 当前支持的模型、Skill 与 MCP 扩展方式。
status: current
audience: user
owner: product
last_verified: 2026-08-23
---

# 扩展与集成

Captain Who 可以通过模型 Provider、Skill 和 MCP Server 扩展能力。这三种方式解决的问题不同：

| 方式          | 适合做什么                | 是否会运行外部程序                         |
| ------------- | ------------------------- | ------------------------------------------ |
| 模型 Provider | 连接兼容的语言模型 API    | 不会；应用通过网络请求模型接口             |
| Skill         | 教 Agent 怎样完成一类任务 | Skill 本身不会；其中的脚本可能需要单独审批 |
| MCP Server    | 给 Agent 增加外部 Tool    | 会；当前支持本机 stdio 进程                |

建议先从内置能力开始。只有当任务需要特定工作流、私有工具或其他模型端点时，再增加扩展。

## 从这里开始

- [集成方式概览](overview.md)：选择 Skill、MCP 还是模型 Provider。
- [开发 Skill](skill-development.md)：为项目编写可复用的 Agent 指令包。
- [连接 MCP Server](mcp-server-integration.md)：连接本机 stdio MCP Server。
- [连接模型 Provider](model-provider-integration.md)：配置模型 API 和兼容模式。
- [兼容性](compatibility.md)：查看当前支持范围和明确不支持的能力。

## 安全提醒

第三方 Skill、MCP Server 和模型服务均不由 Captain Who 控制。安装或连接前应核对来源；不要把 Token、Cookie 或密码写入 Skill 文件、MCP 启动参数、问题截图或公开日志。扩展的名称和说明也不能替代权限与审批。
