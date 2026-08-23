---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-08-23
---

# 恢复与故障处理 Runbook

本文用于开发和维护环境中的 Core Server、SQLite、Multi-Agent、MCP 与 Managed Playwright 故障。优先原则是保护持久事实和外部副作用，不通过手工改库“修绿”。

## 1. 先确定故障域

| 现象                                                                 | 首要检查                                                                              | 不要先做                                      |
| -------------------------------------------------------------------- | ------------------------------------------------------------------------------------- | --------------------------------------------- |
| Core Server 无法启动，含 `development_storage_schema_reset_required` | schema version/fingerprint、数据根、完整错误码                                        | 手工改 `PRAGMA user_version` 或删表           |
| 提示 exact DB 已被占用                                               | 是否有 MyCopilot/Core Server 进程仍运行；实例锁路径                                   | 删除 lock file 后继续运行两个 Core Server     |
| 子 Agent 长时间 queued/claimed                                       | Core Server 是否重启、Dispatcher 是否运行、lease deadline、数据库可写                 | 重复 spawn 同一任务或直接把 Wake 改 completed |
| 子 Agent 显示 running，但进程已崩溃                                  | trace/checkpoint/pending action/lease                                                 | 盲重放可能已有外部副作用的 Turn               |
| 子 Agent 等待审批                                                    | 根 Agent Approval projection、原 pending action、expiry                               | 从子 Agent 页面直接写 decision                |
| UI 丢活动或串线                                                      | 根 Agent ID、event sequence、resync、重新 hydrate                                     | 从 notification 时间戳重建状态                |
| 用户 MCP Server 无法启动                                             | Registry status、launch authorization、program/cwd identity                           | 重用旧授权或把 Token 填入 argv                |
| Managed Playwright 卡住                                              | bridge pending、dispatch certainty、Browser surface/target、Core Server/Main shutdown | 开 remote-debugging 后注入完成结果            |

处理前记录：commit SHA、OS/arch、开发或打包模式、数据根、Core Server stderr 的脱敏错误码、触发步骤和最后一次成功命令。不要记录 API Token、MCP 原始参数/结果、Cookie 或审批 ciphertext。

## 2. 通用安全步骤

1. 停止新的用户操作和 release gate；不要让第二个 Core Server 指向同一数据库。
2. 优先正常退出应用，使 `core.shutdown`、Dispatcher、MCP Manager 和 outbound writer 有机会收口。
3. 若进程已退出，保留原 `storage.sqlite`、`storage-backups/` 和相关日志；不要立即覆盖。
4. 区分“可从 SQLite 自动恢复”和“需要显式开发 reset/人工确认”的故障。
5. 在副本或仓库 fixture 上复现。不要让测试命令打开或迁移真实用户数据库。

## 3. Schema/reset-required

### 判断

当前唯一可接受基线为 **schema v18 + exact catalog fingerprint + valid foreign keys**。真源：

```text
crates/core/src/storage/migrations.rs
crates/core/src/storage/canonical_schema.sql
```

下列情况都返回 `development_storage_schema_reset_required`：旧版本、非空未版本化库、当前版本但 catalog 漂移、外键违规。开发策略是不原地迁移，也不自动重写源库。

### 预检

完全退出 MyCopilot 后运行：

```bash
pnpm storage:reset-dev
```

默认是只读 dry-run。它通过 Electron 解析与应用相同的 `userData`，取得 exact database instance lock，并只输出路径和计数。若提示数据库仍被占用，返回应用/进程排查；不要删除 lock 文件绕过 owner。

### 确认重建

阅读 dry-run 摘要后显式执行：

```bash
pnpm storage:reset-dev -- --confirm-reset
```

确认流程：

1. 在 `storage-backups/` 创建权限受限、时间戳命名的 verified SQLite snapshot。
2. 从只读源或临时副本提取 allowlisted configuration；MCP 精确 identity/authorization 也要重新验证。
3. 在 staging 文件创建 fresh v18 canonical DB。
4. 通过当前 service 写路径恢复配置。
5. 重开生产 storage，核对记录数、`PRAGMA quick_check` 和 `foreign_key_check`。
6. 原子发布新数据库；失败时保留原数据库与恢复备份。

保留内容：

- model/provider settings（含当前代码仍以本机 SQLite 明文保存的模型 Token/Tavily key）；
- UI preferences、Agent prompt preferences；
- Skill enablement overrides；
- 默认 image-generation profile；
- MCP Registry、model namespace、仍有效的 launch authorization 与 enabled/trust 状态。

不恢复：Conversation、Project、message、draft、Usage、Approval、Continuation、Compaction、Fork、Agent tree/Mailbox/Wake 等会话派生状态。附件、已安装 Skill、生成图片和 credential 目录不在 reset 事务中移动；没有数据库引用的附件可在后续正常启动时被孤儿清理。

### 备份处理

- reset 失败时，优先保留原库；backup 是恢复/取证副本，不应被 reset 检查过程修改。
- 不要直接把一个旧 schema backup 覆盖回运行路径并期待 v18 接受；旧库仍会触发 reset-required。
- 如必须人工还原文件，先停止所有 Core Server、再次复制保存当前文件、在隔离位置验证 SQLite 完整性和 schema，再决定是否替换。仓库当前没有受支持的一键 backup restore 命令。
- 不要把包含本机模型 Token/搜索 key 的 backup 上传到 issue、CI artifact 或公共对象存储。

## 4. Multi-Agent 自动恢复

SQLite 是 Wake、Turn、Mailbox、receipt、Approval 和 event 的恢复真相。正常情况下，只需重新启动同一数据根的 Core Server：bootstrap 会在接收请求前启动 Dispatcher，并立即/周期扫描 queued 与 recoverable Wake。

### Wake 分类

| 持久状态                                      | 恢复动作                                                          |
| --------------------------------------------- | ----------------------------------------------------------------- |
| `queued`                                      | 按 FIFO 等待 Dispatcher claim                                     |
| 过期 `claimed`，无 Run identity               | 安全重新排队/重新 claim；确定的 admission 前失败可结算 failed     |
| `running`，已有 terminal trace                | 观察 terminal fact，完成 result outbox/owner 释放，不重跑 Runtime |
| `waiting_for_approval` 且 pending action 存在 | 保持等待；根 Agent UI 从 durable projection 恢复                  |
| `running` 且有合法 checkpoint                 | 按原 identity/permissions/capability snapshot 恢复                |
| `running` 且无可证明安全的 checkpoint         | 结算 `outcome_unknown`，不重放 Tool/Provider 副作用               |

Wake lease 为 60 秒，Dispatcher 每 20 秒续租、每 1 秒 durable fallback scan。新 Core Server 启动时旧 lease 尚未过期属于正常窗口；周期扫描会在 deadline 后处理，不需要再次重启。

### `outcome_unknown`

`outcome_unknown` 表示系统无法证明外部动作未发生，也没有权威响应。处理方式：

1. 保留原 Run/Wake/trace 与错误分类。
2. 在外部系统检查是否已发生操作，例如文件、远端 MCP、命令或第三方 API 状态。
3. 不直接 retry 原 tool call。若业务上需要补偿或重新执行，由用户创建新的显式任务，并说明已核验的外部状态。
4. 若可建立确定性恢复协议，应作为实现变更加入 receipt/checkpoint/fault-injection，而不是在 Runbook 中约定手工改状态。

### Approval

- 子 Agent Approval 的 source of truth 是原 `agent_pending_actions` 与 checkpoint；根 Agent projection 只是授权视图。
- 重复、过期或已完成 decision 应返回 durable settlement，不会启动第二个 continuation。
- Approval 在 shutdown 快照后恢复的竞态由 Dispatcher 在 grace window 内重复扫描；未收口事实留给下次启动恢复。
- 凭据 backend 不可用导致 process-only MCP approval 时，重启后无法恢复原始参数；应让原 action 明确终结，再由用户重新发起。

## 5. 通知、observer 与 UI 恢复

以下 notification 都只是 invalidation：

- `agent.collaboration.event`
- `agent.collaboration.observerEvent`
- `agent.collaboration.resync`

出现缺卡、重复卡、顺序跳跃或 Core Server 重启时：

1. 核对当前 `rootConversationId`，立即丢弃其他根 Agent 的 notification。
2. 从 Host 重新获取 tree snapshot 和保守 replay cursor。
3. 使用 `agent.collaboration.listEvents(afterSequence)` 按根 Agent 本地 sequence 补洞。
4. 遇到 gap/乱序或 `resync` 时完整 rehydrate；不要用 occurredAt 排序覆盖 sequence。
5. 子 Agent observer 再加载 authoritative Conversation snapshot；live overlay 只用于低延迟，不替代持久消息。

页面刷新或进程内 `Notify` 丢失不应造成事实丢失。如果重新 hydrate 仍无法收敛，应优先视为数据库/协议 bug并保存 fixture，而不是清空 Renderer store 后宣称恢复成功。

## 6. MCP stdio 恢复

1. 从 MCP management status 获取 typed error，确认是 config、authorization、negotiation、catalog、timeout、transport close 还是 unknown outcome。
2. executable/cwd/code identity 改变时 authorization 应变 stale；通过原生预览重新授权，不复用旧 digest。
3. MCP Server crash 后 Manager 可以重连/重启连接，但不会自动重放可能发出的 Tool call。
4. stop/delete/shutdown 后在 Unix 检查 repository-owned fixture PID 是否消失；Windows 当前不能假设完整进程树已回收。
5. Catalog incomplete 时修复 cursor/schema/limit 问题并显式 refresh；不要调用部分 Catalog 中的 Tool。
6. 不把 stderr、Tool args/result 或凭据复制到普通诊断日志。只保留 safe error、server ID、revision 和 certainty。

## 7. Managed Playwright 恢复

- bridge request 的 dispatch phase 只能单调前进。已 `possibly_dispatched` 但没有完成响应的 Tool 必须按 unknown outcome 处理。
- Core Server shutdown 期间仍要让 Main 回传 `mcp.builtinPlaywright.dispatchPhase`/`mcp.builtinPlaywright.complete`；不要先切断 stdin 或添加外部 completion hook。
- Browser target/surface 崩溃由 Main broker 和 generation recovery 处理；不得复用用户浏览器 profile、remote debugging 或 DevTools 注入绕过生产信任边界。
- idle runtime 可在 10 分钟后关闭；下一次允许调用再按管理生命周期重建。
- packaged startup gate 不提供真实 Agent 驱动能力，不能用它排除业务链路故障。

## 8. 强制终止与关停超时

正常 `core.shutdown` 的顺序是：停止/取消 Browser risk → Managed Playwright runtime settlement（期间只继续接收反向 bridge 收口请求）→ MCP management stop admission → expiry reconciler → 各 dispatcher、图片执行、Multi-Agent Dispatcher、active Run、MCP tasks/Manager 有界并行收口 → 返回 shutdown response → drain outbound。

当前 Main watchdog 为 6 秒；Multi-Agent Dispatcher 的单阶段 `shutdown_grace` 为 5 秒，且算法最多经历“等待自行完成”和“取消后再扫描”两个阶段，理论最坏接近 10 秒。若 Main 先到 6 秒并强制终止 Core Server，不得把它记录为优雅关停；应保留 SQLite，并按 Wake/trace/checkpoint 分类恢复。

若 Main 的 shutdown watchdog 或子系统 grace 到期：

- 已接受但未完成的异步任务由 owner abort/join 或保留 durable recovery fact；
- 不应伪造成功终态来释放数据库 owner；
- 下一次启动按上述 reconciliation 分类；
- 连续复现应保存具体 subsystem shutdown report，并添加确定性测试，不能无限扩大 watchdog 掩盖泄漏。

## 9. 代码真源

- schema/reset：`crates/core/src/storage/migrations.rs`、`crates/core-server/src/bin/storage-reset-dev.rs`、`scripts/reset-dev-storage*.mjs`
- 数据库锁：`crates/core/src/storage/database_instance_lock.rs` 及 bootstrap 使用点
- Dispatcher/recovery：`crates/core-server/src/application/agent_dispatcher.rs`、`crates/core/src/storage/agent_graph_repository.rs`
- wait：`crates/core-server/src/application/agent_wait.rs`
- Approval/startup reconciliation：`crates/core-server/src/application/agent`、`transport/bootstrap.rs`
- MCP lifecycle：`crates/mcp-client/src/manager`、`transports/stdio.rs`
- managed bridge：`crates/core-server/src/application/mcp/managed_playwright_bridge.rs`、`src/main/core/managedPlaywrightBridge*`
- Renderer replay：`src/renderer/src/features/agentCollaboration`

## 10. 测试

```bash
pnpm test:storage-reset-dev
cargo test -p mycopilot-core --lib storage::migrations::tests
cargo test -p mycopilot-core-server application::agent_dispatcher
cargo test -p mycopilot-core-server application::agent_wait
pnpm test:multi-agent-release -- --smoke-only
cargo test -p mycopilot-mcp-client --test stdio_integration
pnpm exec vitest run --project managed-playwright-e2e
```

reset 变更必须覆盖 dry-run 只读、锁拒绝、备份不变、失败保留原库、配置保留矩阵、MCP identity、`quick_check`、foreign keys 和临时文件清理。恢复变更必须覆盖 lease deadline、重启、Approval race、unknown outcome 和通知 gap。

## 11. 当前限制

- 仅提供开发 reset，不提供生产原地迁移或通用 backup restore 工具。
- 备份没有自动 retention、加密或异地复制；其中可能含明文本机模型/搜索凭据。
- `outcome_unknown` 没有通用自动补偿，需要核验外部状态后显式发起新任务。
- Windows MCP stdio 没有 Job Object 进程树隔离保证。
- 通知正确性依赖 SQLite replay/polling，当前没有外部运维 dashboard 或 alert。
- 没有 CI 自动运行恢复演练，也没有统一命令验证真实用户数据根；所有测试必须继续使用临时 fixture。
- Packaged Agent→Managed Playwright 的离线 E2E 尚未建立。

## 12. 变更检查表

- [ ] 是否先保护并记录原数据库/backup，且未手工改 schema 或状态？
- [ ] 新 schema 是否更新 version、fingerprint、fresh/legacy/tamper/reset tests？
- [ ] reset preservation allowlist 是否有明确安全理由和精确计数验证？
- [ ] 新长期任务是否有 durable identity、owner、lease、checkpoint 和 startup reconciliation？
- [ ] 新外部副作用是否在无法证明安全时进入 `outcome_unknown`？
- [ ] 通知丢失后是否仍能从 authoritative snapshot/event log 收敛？
- [ ] Approval 与子 Agent observer 是否仍经根 Agent authority，不新增旁路？
- [ ] shutdown 是否停止 admission、有界收口并保留未完成事实供重启恢复？
- [ ] 是否补充故障注入、重启、锁、跨进程与目标平台测试？
- [ ] 是否更新本 Runbook、发布门禁和相应子系统文档？
