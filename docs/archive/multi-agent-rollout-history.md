---
status: historical
audience: developers/maintainers
owner: engineering
last_verified: 2026-08-23
---

# Multi-Agent Rollout 历史

> 本文归档早期“六轮实施路线”和当时的阶段性设计。它不描述当前发布状态、schema 兼容性或运行时配置。当前规范请阅读 [Multi-Agent 当前架构](../architecture/multi-agent.md) 与 [发布门禁](../operations/multi-agent-release-gate.md)。

## 1. 归档目的

Multi-Agent 最初按六轮拆分，是为了在不复制 Agent Loop、不扩大为通用 workflow 的前提下，逐步引入持久 Graph、统一执行、调度恢复、模型工具、前端 observer 和发布故障注入。

随着六轮能力陆续落地，继续把路线图放在“当前架构”中会产生两个问题：

- 已实现能力仍被读成未来计划；
- v7/v8/v10/v11 等阶段性 schema 标签被误读为当前兼容承诺。

因此本文件只保留设计演进与决策背景。当前事实始终以代码和 current 文档为准。

## 2. 最初的核心命题

早期设计先冻结了以下方向，后续实现基本保持：

1. 节点只有 Agent，边只有父子指挥关系；不建设通用 DAG。
2. 一个 Agent 绑定一个独立持久 Conversation；Turn/Run 与 Agent 身份分离。
3. Agent 间只通过持久 Mailbox 通信；SQLite 是恢复真相。
4. 协作调度器只领取 Wake，并调用既有 Agent Loop；不复制 MCP、Skill、Command、Approval、Context 或 Usage。
5. 用户只操作根 Agent；子 Agent 复用聊天展示但保持 observer-only。
6. 外部副作用一旦可能发生就不能盲重试，必须记录确定结果或 `outcome_unknown`。

这些命题从 rollout 假设演化为当前架构不变量。

## 3. 六轮路线回顾

### 第 1 轮：持久化护栏与模板后端

目标是先建立不会被 UI 或进程生命周期绕过的领域事实：

- Agent tree、父/根 Agent、Project、task path 约束；
- AgentNode、MailboxMessage、WakeRequest、Result/Outbox、AgentTemplate；
- 根 Agent 对旧 Conversation 的幂等懒物化；
- structured message origin，使 `role=user` 的 Agent 输入仍保留真实 actor；
- template CRUD、machine key、模型 snapshot 与 request id 幂等；
- repository 原子性、外键、唯一索引和配额测试。

这一轮有意不运行子 Agent、不接 Renderer。设计重点是先让“不完整子 Agent”“重复投影”“跨树消息”和“Wake 永久卡住”等状态无法写入。

### 第 2 轮：统一 Turn 执行器与子 Agent context

目标是让根 Agent 和子 Agent 复用同一条服务端执行路径：

- 从根 Agent chat 路径提取统一 Turn executor；
- collaboration-owned 原子 spawn/fork/bind 用例；
- `fork_turns=none | all | N` 的已结算逻辑轮次 snapshot；
- immutable provenance、附件副本、Artifact grant 和 context snapshot；
- 可信 `AgentRunContext` 与子 Agent identity prompt overlay；
- 基于父/祖先持久 snapshot 的权限收紧；
- SQLite durable active trace、Conversation revision CAS 和 per-Agent 单 Turn；
- Approval continuation 对 run/context/lease 的 exact revalidation。

路线明确拒绝两条捷径：不能用普通 create 放宽子 Agent Conversation 新鲜度，也不能为子 Agent 复制一套 prompt builder/Runtime。

### 第 3 轮：Mailbox 运行时、Dispatcher 与双等待

这一轮把持久事实接到可靠执行：

#### Mailbox 与 deferred Wake

- `send_message` 只入队；follow-up 在同一事务创建 deferred Wake；
- source FIFO projection/ack 与 Wake claim 置于同一事务；
- terminal result、parent outbox/Wake 原子提交；
- typed result envelope 绑定子 Agent、Wake、Run、终态与 Artifact refs；
- Mailbox 的 1,024/960 条与 16/15 MiB 背压。

#### Dispatcher 与恢复

- 全局有界并发、每 Agent 单 Turn、共享根/子 Agent concurrency gate；
- queued → claimed → running/waiting/terminal 的 lease 与 CAS；
- startup + periodic recovery，覆盖新进程启动时旧 lease 未过期的窗口；
- shutdown 停止 claim、有界取消、未收口事实留 SQLite；
- terminal trace、Approval checkpoint、pre-Runtime failure 与 `outcome_unknown` 的分类恢复。

#### 安全采样边界

- 每次 Provider 采样前从 Host inbox 获取动态协作事实；
- `(run_id, model_batch_index)` receipt、item 唯一性、model-context/trace 同事务；
- 根 Agent 空闲时，子 Agent result 不在后台启动根 Agent 模型，而在下一次安全采样进入；
- Host-authenticated Agent envelope，不冒充 human；
- 有界 batch、确定性截断与超预算延后。

#### `wait_agent` 内核

- first-ready、最多 32 targets、SQLite check/register/recheck；
- `Notify` 只优化延迟，50ms durable polling 保证正确性；
- wait cursor、target snapshot、ToolResult/model-context prefix 预提交；
- Agent wait 与 Command Session wait 两个独立停止/结果域。

最初文档曾写“本轮只实现内部 WaitKernel，不注册模型 Tool”。该状态在第 4 轮 Harness 接线后失效，源码里残留的同类注释也不能作为当前状态。

### 第 4 轮：六工具、授权、审批与跨进程协议

这一轮把运行时安全地暴露给模型与 Host：

- 精确六工具：`spawn_agent`、`send_message`、`followup_task`、`wait_agent`、`list_agents`、`interrupt_agent`；
- Rust Core 只持有 schema 与 `AgentCollaborationExecutor` port；Core Server Harness 注入可信 caller；
- 每 Turn 冻结脱敏 selector 目录，精确匹配模板/模型和 `imageInput`；
- `CollaborationAuthorizer` 统一 Harness、RPC 和旧 Conversation 写入口；
- 仅根 Agent 可执行 user interaction；子 Agent observer 使用精确根/子 Agent identity；
- 子 Agent Approval 的根 Agent projection、幂等 decision 和原 checkpoint continuation；
- `agent.collaboration.*` RPC、根 Agent 本地 event outbox 和通知补洞；
- Dispatcher 在 request admission 前启动，真实 fake-provider 六工具集成测试。

这轮还确立了“通知是 invalidation、不是 state truth”，以及 notification 精确前缀 `agent.collaboration.*`。

### 第 5 轮：Renderer 复用与完整体验

目标是在不复制聊天业务的前提下展示子 Agent：

- `ConversationSurface` 拆分 interactive/observer capability contract；
- observer 复用 message、Markdown、Tool/MCP/Skill、Artifact、error、Usage 和同一 reducer；
- AppShell 只持有一个根 Agent 范围的 `CollaborationStore`；
- tree/activity 从 snapshot + event replay 恢复；observer live envelope 只作低延迟 overlay；
- backend-authored semantic activity 放入根 Agent Timeline，不从模型文案猜测；
- 根 Agent collaboration fork 保留 provenance，过滤 Agent-origin 的用户可见历史/搜索；
- 子 Agent Approval、右侧 Agent Center、根 Agent switching 和 reload 场景测试。

### 第 6 轮：可靠性、schema 与发布门禁

最后一轮集中关闭跨层故障：

- crash/restart、duplicate request、lease expiry、outbox failure、notification gap/resync；
- Approval race、模型不可用、删除/权限绕过、Command/Agent 双等待；
- 64/65 节点、10,000 facts、32 Agents/并发 4、500 turns、20 recoveries、1,000 subscriptions；
- schema mismatch/reset-required、backup/reset fault injection；
- Rust/TypeScript protocol fixture 与 Renderer browser scenarios；
- 组合 `pnpm test:multi-agent-release` runner 和结构化 `ROUND6_*` 证据。

“第 6 轮完成”只代表这组 Multi-Agent 专项证据建立，不代表 package、真实签名、notarization 或 Managed Playwright packaged E2E 自动纳入同一 gate。

## 4. Schema 标签的历史语义

旧路线文档曾记录阶段性 canonical baselines：

- v7：早期协作协议/事件基线；
- v8：provenance-aware 根 Agent collaboration fork；
- v11：持久权限 snapshot 与 semantic activity projection。

这些数字只标记当时开发 baseline。当前版本不得从本归档推断；截至本归档最后核对时，current 文档与代码真源记录为 **v16**，且只接受 exact current fingerprint。

仓库采用开发 reset-only 策略，因此中间版本不是支持的升级链。任何 v7/v8/v10/v11 文本都不能用作兼容或恢复判断。

## 5. 路线落点索引

下表只解释历史工作包后来落在何处，帮助阅读旧提交；它不是当前模块的完整清单，也不承担路径和接口的持续维护责任。

| 历史工作流                   | 当前代码落点                                                                                            |
| ---------------------------- | ------------------------------------------------------------------------------------------------------- |
| Graph/Mailbox/Wake/receipt   | `crates/core/src/storage/agent_graph_repository.rs`、canonical schema                                   |
| 统一 Turn 与子 Agent context | `crates/core-server/src/application/agent`、Rust Core context/checkpoint                                |
| Collaboration service        | `crates/core-server/src/application/agent_collaboration.rs`                                             |
| Dispatcher/recovery          | `crates/core-server/src/application/agent_dispatcher.rs`                                                |
| Wait                         | `crates/core-server/src/application/agent_wait.rs`                                                      |
| 六工具 Host adapter          | `crates/core/src/agent_collaboration_harness.rs`、`crates/core-server/src/application/agent_harness.rs` |
| 授权                         | `crates/core-server/src/application/collaboration_authorization.rs`                                     |
| RPC/事件                     | `crates/protocol-rs/src/agent_collaboration.rs`、`packages/protocol/src/agentCollaboration.ts`          |
| Renderer observer/store      | `src/renderer/src/features/agentCollaboration`、`ConversationSurface`                                   |
| Release profile/smoke        | `scripts/run-multi-agent-release-gate.mjs`                                                              |

路线收尾时，bootstrap 已在接收请求前启动 collaboration Dispatcher，Harness 和 Wait 也已接线。源码中当时仍存在“deferred to round 4”一类历史注释；今天的接线状态必须从 current 架构文档和代码判断。

## 6. 保留下来的设计取舍

- tree 而非 DAG；
- durable Mailbox 而非内存 actor message；
- receipt/outbox 而非“通知即成功”；
- unified Agent Loop 而非子 Agent runtime fork；
- 仅根 Agent authority 与 observer-only 子 Agent；
- exact selector/caller identity 而非模型声明身份；
- first-ready wait 而非 workflow quorum DSL；
- `outcome_unknown` 而非重试未知副作用；
- current-only development schema + explicit reset，而非未设计完的自动 migration。

如果未来要改变这些取舍，应写新的 current architecture/ADR，并说明兼容、迁移和 release gate；不能直接改写历史让旧路线看似从未存在。

## 7. 当前真源导航

历史材料来源为目录重构前的顶层多智能体架构与 release-gate 文档；原文仍可从 Git 历史追溯。本归档不维护当前代码真源清单。请从以下 current 文档进入：

- [Multi-Agent 当前架构](../architecture/multi-agent.md)
- [Core Server 架构](../architecture/core-server.md)
- [Multi-Agent 发布门禁](../operations/multi-agent-release-gate.md)
- [测试策略与矩阵](../development/testing.md)

本文件出现的旧 schema、路径、数值和轮次文字均为历史引用，不能覆盖上述 current 文档所指向的代码真源。

## 8. 当前验证入口

本归档不复制当前测试命令，避免历史文件成为发布入口。验证今天的实现时，应使用 [Multi-Agent 发布门禁](../operations/multi-agent-release-gate.md) 与 [测试策略与矩阵](../development/testing.md) 中的命令。

## 9. 归档边界

- 本文不是 current design、操作 Runbook、schema migration guide 或 release checklist。
- 六轮边界是当时的工程拆分，并非今天的模块版本或支持等级。
- 旧文档中的 intermediate schema、notification 名或“尚未接线”说明可能与当前代码冲突。
- 本文不维护每次 commit 的完整 changelog；只保留影响架构理解的阶段性决策。
- 未来功能不能继续追加“第 7 轮”来代替 current architecture 和明确 ADR。

## 10. 变更检查表

- [ ] 变更是否真的是历史澄清，而不是应写入 current 架构/Runbook 的新事实？
- [ ] 是否保留原阶段语境，并明确旧 schema/状态不构成当前承诺？
- [ ] 是否链接到当前代码真源与 current 文档？
- [ ] 是否避免根据记忆重写历史，且有旧文档、测试或提交证据？
- [ ] 新架构决策是否另有 current 文档/ADR、迁移和 release gate？
- [ ] 是否未把新的发布状态、当前限额或操作命令只写在 archive 中？
