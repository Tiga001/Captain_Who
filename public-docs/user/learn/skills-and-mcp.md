---
title: Skill 与 MCP
description: 区分“教 Agent 怎样做”的 Skill 和“给 Agent 新工具”的 MCP。
status: current
audience: user
owner: product-docs
last_verified: 2026-08-31
---

# Skill 与 MCP

Skill 和 MCP 都能扩展 Agent，但解决的问题不同：**Skill 主要提供可复用的方法与资源，MCP Server 主要提供可调用的外部工具。**

| 对比         | Skill                              | MCP Server                                    |
| ------------ | ---------------------------------- | --------------------------------------------- |
| 核心作用     | 教 Agent 按某种流程工作            | 提供新的可执行 Tool                           |
| 典型内容     | 指令、参考、模板、资源、可选脚本   | 工具目录、参数结构和调用结果                  |
| 生命周期     | 在一次 Run 中发现和激活            | 本地进程连接后提供工具目录                    |
| 当前来源     | 应用内置、用户安装、项目 Workspace | 用户配置的本地 stdio Server；另有应用内置能力 |
| 是否自动授权 | 否                                 | 否                                            |

## Skill：把经验做成包

一个 Skill 以 `SKILL.md` 为入口，可以带参考资料、模板、资产和脚本。Agent 先看到简短目录，选中后才加载具体说明，以减少不必要的上下文占用。

三类来源有不同范围：

- **Bundled**：随应用提供，例如文档、表格、演示、PDF、图片生成、Skill Creator；
- **Installed**：用户从 GitHub 或已授权本地目录检查并安装，供全局使用；
- **Workspace**：位于当前项目 `.agents/skills/`，只在该项目中发现。

激活通常只对当前 Run 有效。包在激活时绑定到一个精确版本；运行中修改源文件不会悄悄替换已加载的说明。

Skill 的“可信来源”也不等于权限。写文件、运行脚本、生成 Office 文档仍要经过各自校验。当前 Skill Script 只支持受约束的 Python 路径并要求较高权限；Installed/Workspace 脚本始终逐次批准，只有重新验证的应用内置脚本可在内置执行策略允许时自动批准。系统不会自动安装缺失依赖。

## MCP：把工具接进 Agent

MCP 是一种让应用发现和调用外部工具的协议。当前用户可添加的 MCP Server 是本地 stdio 进程：Captain Who 启动它、读取完整工具目录，再把合规工具加入 Agent 的可用集合。

接入有四个不同动作：保存配置、授权启动、启用连接、批准具体工具调用。它们不能合并为一个“永远信任”开关。第三方提供的工具描述和只读标记也不能单独降低安全要求。

Captain Who 的内置浏览器自动化在底层也使用受管 MCP 能力，但它由应用固定并通过专门的浏览器风险门禁运行，不是用户添加的任意 Server。

## 什么时候选哪一个

- 团队有固定报告格式：创建 Skill。
- 需要调用本地数据库或专用服务工具：连接 MCP Server。
- 需要“先按团队流程判断，再调用专用系统”：可以同时使用 Skill 和 MCP。
- 只是一次简单任务：直接写清 Prompt，避免为了扩展而扩展。

## 两者都不能做什么

- 不能凭自身说明扩大 Agent 权限；
- 不能绕过文件路径、命令、网络或审批策略；
- 不能保证第三方内容可信；
- 不能自动解决模型上下文和外部副作用的不确定性。

动手学习：[创建 Workspace Skill](../tutorials/create-a-skill.md)或[连接本地 MCP Server](../tutorials/connect-an-mcp-server.md)。
