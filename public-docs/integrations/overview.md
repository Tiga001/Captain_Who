---
title: 集成方式概览
description: 根据需求选择模型 Provider、Skill、MCP Server 或内置能力。
status: current
audience: user
owner: product
last_verified: 2026-08-23
---

# 集成方式概览

扩展 MyCopilot 前，先判断你要增加的是“思考能力”“工作方法”还是“可调用工具”。

## 如何选择

| 你的目标                  | 推荐方式         | 例子                                                      |
| ------------------------- | ---------------- | --------------------------------------------------------- |
| 使用另一个模型或 API 网关 | 模型 Provider    | OpenAI-compatible、Anthropic-compatible、DeepSeek V4 Chat |
| 固化一套可复用的操作方法  | Skill            | 代码审查规范、报告模板、团队工作流                        |
| 调用已有系统或本地服务    | MCP Server       | 数据查询、内部工具、专用自动化                            |
| 自动操作网页              | 内置浏览器自动化 | 在受管浏览器中查找、点击、填写或下载                      |

Skill 可以告诉 Agent 何时调用 MCP Tool；MCP Server 也可以配合某个模型使用，但它们不会自动相互授权。每一层仍受当前任务的权限、审批和可用性约束。

## 集成边界

### 模型 Provider

模型请求会发送到你配置的 API URL。端点“看起来兼容”并不保证完整支持 Tool 调用、流式响应、Usage 或推理续接；配置后应先用低风险任务验证。

### Skill

Skill 是包含说明和可选资源的内容包。项目内的 Workspace Skill 只对当前项目可见；已安装 Skill 由应用管理，可在设置中启用、更新或卸载。Skill 的可信来源不等于文件、命令或网络权限。

### MCP Server

当前用户集成只支持本机 stdio MCP Server。保存配置、授权启动、启用连接和批准 Tool 调用是不同步骤。MCP Server 是本机外部进程，能做什么还取决于操作系统授予该进程的权限。

### 内置能力

浏览器自动化等内置能力也使用 MCP 技术，但它们由应用管理，不是用户可编辑的外部 MCP Server。不要尝试用内部连接参数配置第三方 Server。

## 推荐顺序

1. 先用内置 Tool 完成一次任务。
2. 需要稳定流程时创建 Workspace Skill。
3. 只有确实需要外部能力时再连接 MCP Server。
4. 从需要逐次确认的审批模式开始，确认行为后再评估自动执行。
5. 为模型、Skill 和 MCP 分别保留来源、版本和验证记录。

完整支持范围见[兼容性](compatibility.md)。
