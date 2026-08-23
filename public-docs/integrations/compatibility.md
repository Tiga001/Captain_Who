---
title: 集成兼容性
description: 查看 MyCopilot 当前对 Provider、Skill、MCP 和文件能力的支持范围。
status: current
audience: user
owner: product
last_verified: 2026-08-23
---

# 集成兼容性

本页描述当前实现范围，不是对任意第三方产品的认证清单。第三方服务升级后，兼容性也可能变化。

## 模型 Provider

| 能力                                      | 当前状态                   |
| ----------------------------------------- | -------------------------- |
| OpenAI-compatible Chat 与 Tool Calls      | 支持对应通用适配器         |
| Anthropic-compatible Messages 与 Tool Use | 支持对应通用适配器         |
| DeepSeek V4 Chat 推理续接                 | 支持专用配置               |
| 任意 OpenAI/Anthropic 网关                | 不保证；需逐端点验证       |
| 自动发现模型与价格                        | 不支持，模型资料由用户配置 |

## Skill

| 能力                                | 当前状态                              |
| ----------------------------------- | ------------------------------------- |
| 项目 Workspace Skill                | 支持 `.agents/skills/<name>/SKILL.md` |
| 内置与用户已安装 Skill              | 支持设置页管理                        |
| GitHub 与授权本地目录安装           | 支持检查后安装                        |
| references/assets/templates/scripts | 支持经过验证的包资源                  |
| 任意在线 Skill Registry             | 尚未开放                              |
| 常驻跨 Run 激活                     | 不支持                                |

Skill Script 当前仅支持 Python 3 脚本，并要求高权限前置条件和逐次批准。

## MCP

| 能力                                                       | 当前状态                                 |
| ---------------------------------------------------------- | ---------------------------------------- |
| 本机 stdio MCP Server                                      | 支持                                     |
| Tool discovery 与调用                                      | 支持完整、通过校验的 Catalog             |
| Prompt / Auto / Deny 调用策略                              | 后端支持；界面主要提供逐次确认与自动执行 |
| Streamable HTTP / HTTP Server                              | 不支持                                   |
| OAuth / PKCE                                               | 不支持                                   |
| 用户环境变量、SecretRef、headers                           | 不支持                                   |
| Resources、Prompts、Sampling、Elicitation、Tasks、MCP Apps | 尚未接入 Agent 使用路径                  |

内置 Managed Playwright 使用受管 MCP 通道，但这不是可供第三方复用的通用传输。

## 文档和制品

- 附件读取支持 PDF、DOCX、PPTX、XLSX、CSV 和 TSV 等常见格式。
- 旧版 `.doc` 仅在 macOS 上可通过系统能力读取；`.ppt` 和 `.xls` 暂不支持。
- 扫描 PDF 是否可读取取决于文件是否含文本层。
- Word、表格、演示文稿、PDF 和图片生成依赖应用随附的受管组件；具体可用性还取决于安装包平台验证和所需 Provider 配置。

## 平台差异

macOS、Windows 和 Linux 都有原生打包入口，受管组件清单也登记了这些平台的 x64/arm64 资源；当前打包命令并不单独证明每种组合都已构建和验收。主要实测平台仍是 macOS。具体证据边界见[平台与构建状态](../releases/supported-platforms.md)；构建目标不等于公开发行支持。
