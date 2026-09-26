---
status: current
audience: developers
owner: engineering
last_verified: 2026-09-26
---

# 术语表

本文统一代码评审、开发文档和发布说明中的名称。代码标识、协议字段和 UI 文案保持原始拼写；普通
叙述采用下表术语，避免为同一概念另造名称。

## 进程与边界

| 规范名称    | 含义                                                                | 避免使用                              |
| ----------- | ------------------------------------------------------------------- | ------------------------------------- |
| Renderer    | Electron 中的 React 渲染进程                                        | “前端进程”“renderer 层”混用           |
| Preload     | context-isolated bridge                                             | “注入脚本”作为正式名称                |
| Main        | Electron 主进程                                                     | 只写“后端”                            |
| Host        | 持有某项受信能力的宿主；正文应说明是 Main 还是 Core Server          | 把 Host 等同于任意一个进程            |
| Core Server | `core-server` Rust sidecar 及其 application/transport/adapters 边界 | “Rust 后端”“core-server 层”混用       |
| Rust Core   | `mycopilot-core` crate 的领域与运行时实现                           | 用 “Core” 同时指 Core Server 和 crate |
| Host API    | Renderer 经 Preload 调用 Main 的类型化接口                          | “IPC API”泛指所有跨进程协议           |
| JSON-RPC    | Main 与 Core Server 间的逐行 JSON-RPC 2.0 协议                      | 与 Electron IPC 混称                  |

第一次出现时可以写“Core Server（Rust sidecar）”；之后保持 `Core Server`。路径、binary 和 package 名称
仍写为 `core-server`。

## Agent 运行

| 规范名称            | 含义                                                                               |
| ------------------- | ---------------------------------------------------------------------------------- |
| Agent               | 持有模型上下文、工具能力和生命周期的运行实体                                       |
| 根 Agent / 子 Agent | 持久 Agent 树中的 root/child；不要使用“主智能体/从智能体”                          |
| Subagents           | 仅指设置页或 UI 中的产品名称                                                       |
| Conversation        | 持久对话容器；不等同于一次模型请求                                                 |
| Turn                | 一次用户或 Wake 驱动的 Agent 执行周期                                              |
| Run                 | Turn 的可执行/可观察运行实例；引用 DTO 时保留具体 `runId` 语义                     |
| Agent Runtime       | Provider、上下文、Tool Loop、Checkpoint 和结算的运行边界                           |
| Wake                | 使子 Agent 可被 Dispatcher 调度的持久事实                                          |
| Mailbox             | Agent 间任务、消息、跟进和结果的持久投递域                                         |
| Dispatcher          | 领取 Wake、获取并发许可并启动子 Agent Turn 的进程级调度器                          |
| Approval            | 对一项冻结 action 的用户决定；不是一般配置权限                                     |
| Permission          | 当前任务/工作区允许的能力范围和自动审批策略                                        |
| Automation          | 用户配置并由 Core Server 持久调度的 Agent 定时任务；不要用它泛指浏览器自动化       |
| Automation Run      | 一次计划、手动或恢复触发的 Automation 执行实例                                     |
| Attention           | Automation 中需要用户查看或处理的持久提醒投影，不等同于原生通知                    |
| Agent Template      | 工作区模板库中的子 Agent 配置；通过显式项目分配控制可用范围                        |
| Workflow Definition | 设置页编辑的图定义，包含 Agent/用户节点、逻辑门、流股与布局；独立于 Agent Template |
| Workflow Instance   | 全局工作流实例，把定义中的 Agent 节点绑定到对话；开启状态不代表图已开始执行        |
| Workflow Monitor    | 使用原工作流布局观察绑定对话及其子 Agent 活动、审批、提问和未读状态的只读页面      |

中文正文统一写 `Agent`、`Turn`、`Run`，不交替使用“智能体”“轮次”“回合”“执行任务”来指代同一
领域对象。“多智能体”可作为产品能力名称，具体实体仍写根 Agent/子 Agent。

## 模型、工具与扩展

| 规范名称           | 含义                                                                                    |
| ------------------ | --------------------------------------------------------------------------------------- |
| Provider           | 模型协议/服务适配边界                                                                   |
| Model              | Provider 下的具体模型配置                                                               |
| Model Availability | Host 对配置、Provider、凭据和模型能力计算的统一可用性投影；不等同于已启用或网络健康检查 |
| Provider Profile   | Provider dialect、endpoint 能力和默认行为的配置档案                                     |
| Continuation       | Provider 原生的可恢复会话状态；不等同于 Conversation                                    |
| Tool               | Agent Runtime 可调用的结构化工具                                                        |
| FileChange         | Agent 对单个 UTF-8 文本目标执行 create/update/delete 的统一领域动作                     |
| File Observation   | `read_file` 产生并绑定 Run、Conversation、目标与文件版本的写前证明                      |
| Run grant          | 用户在当前 Run 内授予后续同类动作的持久授权；不是永久 Permission                        |
| Skill              | 带 `SKILL.md`、资源和可选脚本的版本化能力包；不要翻译成“技能插件”                       |
| Capability         | 需要显式激活或由 Host 管理的一组能力                                                    |
| MCP Server         | 通过 MCP 提供能力的 peer；用户配置的 Server 当前仅支持 stdio                            |
| HostBridge         | 应用内部、编译期允许的 MCP transport；不是用户 Server transport                         |
| Managed Playwright | Host 管理的内置浏览器自动化链路                                                         |

“外部 MCP”必须写清是“用户配置的 stdio MCP Server”；“内置 MCP”必须写清是 HostBridge/内置
Capability，不能让读者误以为产品已支持用户 HTTP MCP Server。

## 数据与展示

| 规范名称                 | 含义                                                                                          |
| ------------------------ | --------------------------------------------------------------------------------------------- |
| Artifact                 | Host 管理、具有身份、完整性和访问授权的生成制品                                               |
| Generic Managed Artifact | Rust Core 发布的内容寻址受管制品；授权模型不同于 Browser Artifact                             |
| Browser Artifact         | Main broker 管理的短期 Browser 产物引用，例如截图；不是持久下载文件                           |
| Browser Download         | 保存到用户配置目录、由 Rust Core 持久登记并以无绝对路径引用暴露的下载文件                     |
| Attachment               | 用户输入或 Conversation 绑定的附件                                                            |
| Managed Import           | Host 分块接收原文件并持久化完整性信息后返回的 opaque 输入引用；不是文件正文                   |
| Folder Reference         | 用户选定目录的持久引用；名称和绝对路径可提供给模型，读取按目录身份复验，写入仍受 Run 权限约束 |
| Workspace Mention        | Composer 的 `@workspace/<alias>/<path>` 逻辑提及；不导入文件，也不增加目录授权                |
| Workspace Snapshot       | Run 冻结的项目文件夹成员、别名和目录身份；不因后续编辑项目而改写                              |
| Trace                    | append-only 的运行与审计事实流                                                                |
| Exact Archive            | 受限、可分页查询的精确历史归档，不等同于模型活跃上下文                                        |
| Model Context            | 当前一次模型请求实际可见的内容                                                                |
| Notification Fact        | 与业务状态同边界提交的不可变通知事实；不是 Event 或展示文案                                   |
| Notification Batch       | Rust Core 合并、租约和结算的一组待投递通知事实                                                |
| System Notification      | Main 通过操作系统展示的原生提醒；不是业务状态、Approval 或 Attention                          |
| projection               | 从权威状态生成的有界消费者视图；正文可写“投影”                                                |
| snapshot                 | 某个身份/revision 下的只读快照；不表示自动持续同步                                            |
| revision                 | 领域记录的并发/版本身份                                                                       |
| canonical schema version | SQLite canonical schema 版本，真源是 `STORAGE_SCHEMA_VERSION`；当前值和升级边界见存储文档     |
| `schemaVersion`          | 某个 DTO/envelope 自身的协议版本；不能与 SQLite canonical schema 混用                         |

Artifact 首次出现可写“Artifact（制品）”，之后保持 `Artifact`。Generic Managed Artifact、Browser
Artifact、Browser Download 和 Attachment 是四种不同授权模型，不要仅因它们最终可关联文件就混称。

文件输入的完整边界见[对话输入与附件](../subsystems/conversation-inputs.md)，canonical schema 见
[存储与数据生命周期](../architecture/storage-and-data-lifecycle.md)。Workflow 的 `enabled`、`running`、
`needsReview` 分别指实例开关、当前活动投影与配置复核要求；模板的 `enabled` 是校验结果派生的可用性，
不可与实例开关或 Agent Run 状态混称。

## UI 与测试

| 规范名称          | 含义                                                            |
| ----------------- | --------------------------------------------------------------- |
| surface           | 可被 Host 或 Renderer 识别的交互表面，例如 Browser surface      |
| page              | 右侧栏平台中的页面实例                                          |
| transient preview | 可被下一次文件预览替换的临时 Files page                         |
| stable page       | 已固定、不会被下一次 preview 替换的 Files page                  |
| gate              | 必须通过的组合验证；应明确是否属于 `pnpm check` 或发布门禁      |
| smoke             | 验证关键链路能启动/完成的有限测试，不代表完整行为覆盖           |
| profile           | 固定规模、可复现的压力或性能观测配置，不是生产容量承诺          |
| pending           | 尚无足够验证证据；不得在发布说明中改写为“支持”                  |
| Scheduled         | 左侧栏中管理 Automation 的产品入口名；架构正文仍使用 Automation |

## 写作规则

- 标题和正文使用 `Agent`、`Tool`、`Skill`、`Artifact`、`Provider`、`Renderer`、`Preload`、`Main`、
  `Core Server`、`Rust Core`、`MCP Server`、`Managed Playwright`、`Automation` 和 `Automation Run` 的固定
  大小写；`Scheduled` 只作为 UI 名称。
- 引用代码枚举、方法、字段和状态时使用反引号并保持原值，例如 `outcome_unknown`、`schemaVersion`。
- “当前”“已支持”“发布可用”必须有代码和测试证据；计划能力放 ADR proposal 或历史文档。
- 数值、版本和协议名同时给出代码/manifest 真源，不能只依赖本文。

术语发生领域含义变化时，应在同一变更中更新本表、相关权威文档、DTO/代码注释和 UI 翻译；仅修改
大小写或中文措辞不应改变协议字段。
