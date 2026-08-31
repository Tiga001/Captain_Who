---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-31
---

# MyCopilot 开发文档

本目录是 MyCopilot 工程设计、开发流程和发布约束的权威文档入口。根目录
[`README.md`](../README.md) 只负责项目简介和最短启动路径；实现细节以本目录及其链接的代码、契约
fixture 和测试为准。

面向最终用户、集成开发者与外部分发的内容维护在
[`public-docs/`](../public-docs/README.md)；它与本目录分别执行结构和链接检查，不应从公开文档反向链接内部实现说明。

## 新成员阅读顺序

1. [开发环境与启动](development/getting-started.md)
2. [仓库结构](development/repository-layout.md)
3. [系统架构总览](architecture/overview.md)
4. [Agent Runtime 与 Provider](architecture/agent-runtime-and-providers.md)
5. [测试体系](development/testing.md)
6. [术语表](development/glossary.md)
7. 根据负责领域阅读下面的专项文档

## 架构

| 文档                                                                     | 内容                                                 |
| ------------------------------------------------------------------------ | ---------------------------------------------------- |
| [系统架构总览](architecture/overview.md)                                 | 进程、模块、依赖方向和数据流                         |
| [Electron Host](architecture/electron-host.md)                           | Main、Preload、Renderer、sidecar 与窗口生命周期      |
| [Core Server](architecture/core-server.md)                               | application、transport、adapters、启动恢复和停机     |
| [前端架构](architecture/frontend.md)                                     | AppShell、Feature、状态所有权和 Conversation Surface |
| [IPC 与协议](architecture/ipc-and-protocol.md)                           | Host API、IPC、JSON-RPC、DTO 与 fixture              |
| [Agent Runtime 与 Provider](architecture/agent-runtime-and-providers.md) | Agent Loop、Checkpoint、Provider Profile 和切换      |
| [上下文管理](architecture/context-management.md)                         | 上下文组装、预算、压缩和恢复                         |
| [Trace 与历史归档](architecture/conversation-trace-and-archive.md)       | Trace、模型上下文、Exact Archive、Rewrite 与 Fork    |
| [多智能体](architecture/multi-agent.md)                                  | Agent 树、Mailbox、Wake、Dispatcher、审批和恢复      |
| [存储与数据生命周期](architecture/storage-and-data-lifecycle.md)         | SQLite、schema、数据根、备份、重置和删除             |

## 子系统

| 文档                                                              | 内容                                                     |
| ----------------------------------------------------------------- | -------------------------------------------------------- |
| [工具、权限与审批](subsystems/tools-permissions-and-approvals.md) | 工具注册、权限、审批、取消和恢复                         |
| [FileChange](subsystems/file-change.md)                           | `apply_patch`、Observation、审批、提交、审计和历史 Diff  |
| [Scheduled Automation](subsystems/scheduled-automations.md)       | 定时任务、调度、Run、恢复、attention 与通知事实          |
| [通用通知](subsystems/notifications.md)                           | 普通任务与 Automation 的事实、批次、原生投递和点击导航   |
| [Tool Result 消费矩阵](subsystems/tool-result-consumer-matrix.md) | Model、Event、Trace、Archive 等投影消费者                |
| [Tool Result 上限](subsystems/tool-result-limits.md)              | 截断、分页、归档和恢复契约                               |
| [MCP](subsystems/mcp.md)                                          | 用户配置的 stdio MCP Server 与内部 HostBridge Capability |
| [Skills](subsystems/skills.md)                                    | bundled、installed、workspace Skill 生命周期             |
| [浏览器与自动化](subsystems/browser-automation.md)                | surface、Managed Playwright、设置、历史、下载和风险门禁  |
| [命令与会话](subsystems/command-sessions.md)                      | 命令策略、进程组、handoff、wait 和恢复                   |
| [Office 与 Artifact](subsystems/office-and-artifacts.md)          | Word、表格、演示文稿、PDF、受管运行时与 Artifact         |
| [右侧栏平台](subsystems/right-sidebar.md)                         | 模块注册、实例、工作区绑定和页面生命周期                 |
| [终端](subsystems/terminal.md)                                    | node-pty utility process、背压和清理                     |
| [工作区文件](subsystems/workspace-files.md)                       | 路径约束、预览模式和资源预算                             |
| [Git Review](subsystems/git-review.md)                            | diff scope、snapshot、FileChange 历史 Diff 与变更操作    |
| [图片生成](subsystems/image-generation.md)                        | Profile、凭据、任务日志和 Artifact 发布                  |

## 开发、发布与运维

| 文档                                                       | 内容                                            |
| ---------------------------------------------------------- | ----------------------------------------------- |
| [开发环境与启动](development/getting-started.md)           | 前置依赖、安装、开发启动和常用命令              |
| [仓库结构](development/repository-layout.md)               | 目录职责和依赖规则                              |
| [设置与配置](development/settings-and-configuration.md)    | 配置范围、持久化、CAS 和敏感值                  |
| [测试体系](development/testing.md)                         | 单元、集成、浏览器、压力和发布测试              |
| [构建与发布](development/build-and-release.md)             | 平台构建、签名、产物验证和发布清单              |
| [运行时组件](development/runtime-components.md)            | Office、Artifact、Managed Playwright 等冻结组件 |
| [故障排查](development/troubleshooting.md)                 | 启动、存储、MCP、浏览器、Office 和打包问题      |
| [术语表](development/glossary.md)                          | 进程、Agent、工具、数据与测试术语               |
| [文档维护规范](development/documentation-guide.md)         | 文档分类、元数据、评审触发器和完成标准          |
| [多智能体发布门禁](operations/multi-agent-release-gate.md) | 确定性压力与恢复门禁                            |
| [恢复 Runbook](operations/recovery-runbook.md)             | 数据、Agent、命令、MCP 与运行时恢复             |
| [威胁模型](security/threat-model.md)                       | 信任边界、受保护资产和残余风险                  |

## 决策与历史

- [ADR 索引与模板](adr/README.md)：记录仍会影响实现的架构决策。
- [历史文档](archive/README.md)：只保留历史背景，不作为当前实现依据。
- [多智能体分轮落地记录](archive/multi-agent-rollout-history.md)：原六轮实施路线的历史归档。

## 文档状态

- `current`：描述当前代码，可以作为开发和评审依据。
- `draft`：尚未成为工程约束，不得据此宣称功能已实现。
- `historical`：仅用于理解演进，当前行为必须回到 `current` 文档和代码核对。
- `deprecated`：仍为兼容链接保留，内容不再维护。

文档与代码冲突时，先以测试、协议 fixture、注册表和代码常量确认实际行为，再在同一变更中修正文档。
具体规则见[文档维护规范](development/documentation-guide.md)。
