---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-08-23
---

# Multi-Agent 发布门禁

本文冻结 Multi-Agent 的确定性故障注入和压力验证边界。它是发布证据契约，不是容量承诺，也不等于整个应用的发布流程。

## 1. 执行入口

在仓库根目录运行：

```bash
pnpm test:multi-agent-release
```

诊断模式：

```bash
pnpm test:multi-agent-release -- --profile-only
pnpm test:multi-agent-release -- --smoke-only
```

正式证据必须使用默认组合执行；两个诊断模式不能替代完整通过。脚本使用本地 fake Provider、确定性时间输入、临时 SQLite 和严格协议 fixture，不联系付费模型，也不执行真实 MCP、Skill、Command 或文件副作用。

环境变量只允许提高，不允许降低最低压力：

- `MYCOPILOT_MULTI_AGENT_PROFILE_FACTS`：至少 10,000；
- `MYCOPILOT_MULTI_AGENT_PROFILE_RESTARTS`：至少 20；
- `MYCOPILOT_MULTI_AGENT_PROFILE_SUBSCRIPTIONS`：至少 1,000。

## 2. 默认门禁内容

默认运行 4 个 profile step 和 9 个 smoke step，共 13 步。

### Profile

| Step             | 固定边界                                                              | 主要证据                                                  |
| ---------------- | --------------------------------------------------------------------- | --------------------------------------------------------- |
| 持久化压力       | 树 64 节点成功、65 失败；10,000 facts；4 writers；20 crash recoveries | SQLite 原子性、Mailbox/Event 索引、无 busy 泄漏、重开延迟 |
| Dispatcher       | 32 Agents；进程共享并发上限 4                                         | FIFO 进展、每 Agent 单 Turn、无 ghost capacity            |
| Context          | 500 logical turns 后 durable compaction/reload                        | 长历史不会绕过 canonical context/compaction               |
| Renderer/Preload | 1,000 subscribe/unsubscribe 生命周期                                  | 无重复 card/store、订阅可清理、Host bridge 对称           |

### Smoke

| Step                                   | 验证内容                                                                                             |
| -------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| terminal/result/outbox fault injection | terminal、result、父 Agent Wake 的事务回滚与幂等 retry                                               |
| 六工具完整链                           | fake Provider → Tool schema → Runtime → Core Server `AgentHarness` → application service，精确六工具 |
| queued 子 Agent restart                | Core Server 启动后无需新根 Agent Turn 即恢复 queued 子 Agent                                         |
| 双等待域                               | Command Session wait 与 Agent wait 不互相唤醒/消费                                                   |
| 子 Agent Approval continuation         | 进入 Runtime 前持久 `waiting_for_approval → running`                                                 |
| 旧入口授权回归                         | 普通用户 RPC 不能读写子 Agent Conversation                                                           |
| storage migrations tests               | fresh canonical、旧/损坏 schema 拒绝、reset-required 行为                                            |
| cross-language protocol                | Rust 消费协作 fixture，与 TypeScript 契约对齐                                                        |
| AppShell browser scenarios             | activity、Approval、observer、live stream、重启和根 Agent switching                                  |

脚本的 storage step 当前明确标为 “canonical v17”。实际版本的唯一真源仍是 `crates/core/src/storage/migrations.rs`；修改 schema 时必须同步 runner label 与本门禁，不能仅凭日志文字判断兼容性。

## 3. 固定压力阈值

| 维度                   |                                                                                当前 release profile |
| ---------------------- | --------------------------------------------------------------------------------------------------: |
| Tree boundary          |                                                      64 nodes accepted；node 65 atomically rejected |
| Runtime fan-out        |                                                                     32 Agents；global concurrency 4 |
| Durable facts          | 10,000 ordinary Mailbox messages、每个 spawned 子 Agent 的 Result、超过 10,000 根 Agent 本地 events |
| Logical turns          |                                                   500 completed turns + durable compaction + reload |
| Crash/restart          |                                                             20 expired claimed-Wake recovery cycles |
| Subscription lifecycle |                                                 1,000 Renderer/Preload subscribe/unsubscribe cycles |

单 recipient 的 Mailbox 硬上限是 1,024 条/16 MiB；普通消息软上限是 960 条/15 MiB。压力 fixture 因此把 10,000 条消息分散到整棵树，而不是构造产品不会接纳的单 recipient 输入。

正确性阈值为零容忍：不得出现 lost fact、重复 logical result/card、同 Agent 并发 Turn、跨 Conversation stream、逃逸的 SQLite busy、永久 lease 或 ghost active Wake。

脚本记录 wall time、RSS、CPU、enqueue/query/event catch-up/reopen 延迟和数据库大小，但仓库没有正式生产容量预算。这些数字只能用于回归比较，不能对外表述为容量或 SLA。

## 4. 有限故障点清单

| ID  | 边界                                                           | 必须保持的不变量                                               |
| --- | -------------------------------------------------------------- | -------------------------------------------------------------- |
| F1  | `ensure_root`、snapshot、子 Agent Conversation/Agent/task/Wake | 失败无半节点；同 request retry 得到同一身份                    |
| F2  | Mailbox enqueue/claim/projection/ack/deferred Wake             | 事务回滚、FIFO、lease fencing、唯一 projection                 |
| F3  | Dispatcher reserve/claim/Turn admission/renew/shutdown         | 每 Agent 单 Turn、共享全局上限、无 ghost permit                |
| F4  | Provider/Tool/MCP/Skill/Command 外部副作用                     | 只在副作用前安全重试；之后 durable result 或 `outcome_unknown` |
| F5  | terminal trace/Usage → result outbox                           | outbox 失败回滚 settlement；恢复观察而非重放 Runtime           |
| F6  | result outbox → 父 Agent Wake → notification                   | Wake 写失败回滚 result；丢通知从 SQLite 修复                   |
| F7  | wait check/register/recheck/receipt/ToolResult/cursor          | race 只结算一次，未消费事实仍可被后续 wait 读取                |
| F8  | Command wait 与 Agent wait                                     | 两域互不唤醒、互不消费，各自结果仍可查询                       |
| F9  | 子 Agent Approval/create/projection/decision/resume            | 重启、重复、过期决定幂等；先持久状态再恢复 Runtime             |
| F10 | event commit/notification/replay/observer overlay              | duplicate、乱序、gap、resync、reload 最终收敛到 DB             |

新增跨事务或外部副作用边界时，必须扩展这个有限清单及相应 fault injection；不能依赖 sleep 作为正确性证明。

## 5. 协议与端点冻结

模型 Harness 只允许：

- `spawn_agent`
- `send_message`
- `followup_task`
- `wait_agent`
- `list_agents`
- `interrupt_agent`

Renderer/Core Server 的协作 RPC 精确为 `agent.collaboration.getTree`、`agent.collaboration.getAgent`、`agent.collaboration.locateConversation`、`agent.collaboration.loadObserverConversation`、`agent.collaboration.listEvents`、`agent.collaboration.templates.list`、`agent.collaboration.templates.create`、`agent.collaboration.templates.update`、`agent.collaboration.templates.setEnabled`、`agent.collaboration.templates.delete`、`agent.collaboration.approvals.list` 与 `agent.collaboration.approvals.decide`。通知精确为：

- `agent.collaboration.event`
- `agent.collaboration.observerEvent`
- `agent.collaboration.resync`

任何第七个工具、新写端点或通知改名都属于门禁契约变更，必须同时更新 Rust/TypeScript protocol、fixture、Main/Preload allowlist、Renderer 测试和本文。

## 6. Schema 与 reset 门禁

当前 canonical storage 为 **v17**，且只有 normalized SQLite catalog fingerprint 完全匹配才接受。以下输入必须 fail closed 且不修改源库：

- v15 或更早的开发库；
- 非空但 `user_version=0` 的库；
- 当前版本但 schema object 缺失/额外/被篡改；
- foreign key violation。

稳定错误标识为 `development_storage_schema_reset_required`。开发 reset 必须先 dry-run、取得 exact DB lock、创建并验证私有备份、构造 fresh v17、恢复 allowlisted 配置、执行 `quick_check`/`foreign_key_check` 后原子发布。详见 [恢复 Runbook](recovery-runbook.md)。

## 7. 发布所需的组合证据

`pnpm test:multi-agent-release` **只覆盖 Multi-Agent 专项**。它不在 `pnpm check` 中，也不执行打包、真实签名、notarization、付费模型或 packaged Agent→MCP。

准备发布包含 Multi-Agent 的应用时，至少在同一最终源码树上取得：

1. `pnpm check` 完整通过；
2. `pnpm test:multi-agent-release` 默认组合完整通过；
3. schema/reset 测试与显式 dry-run/reset fixture 通过；
4. 目标平台实际 package 构建及 `afterPack/afterSign` 校验通过；
5. 适用的 unpacked/packaged startup smoke 通过；
6. 无未处置的 P0/P1 审计项；
7. 若发布声明包含内置浏览器，再运行 Managed Playwright 专项 gate，并保留 pending 项说明。

仓库目前没有单一 `release:verify` 命令或 CI 把这些步骤绑定到一个 commit。维护者必须记录 commit SHA、平台/架构、工具链版本、每条命令、退出码和产物 hash，避免把不同工作树的通过结果拼接成一次发布。

### Packaged Playwright 限制

`pnpm verify:playwright-packaged-startup` 当前只证明指定的 **unpacked macOS bundle** 可以在隔离 appData 和 loopback deny proxy 下启动，并观察到 Core Server/Renderer 与锁定的 Managed Playwright packages。

它明确不驱动真实 packaged `Agent → Managed MCP Server → local fixture`，状态为 `pending`，reason code 为 `no_production_external_managed_mcp_driver`。没有 public packaged UI + offline Provider fixture + 正常 Approval UI 的证据前，不得把该 gate 宣称为 packaged browser E2E。

## 8. 证据与失败处理

脚本输出：

- `ROUND6_ENV`：OS、架构、Node、CPU/memory 和实际阈值；
- `ROUND6_STEP_START/RESULT`：每步命令、退出码与 wall time；
- heavy step 的 OS resource statistics；
- 测试产生的 `ROUND6_METRIC`；
- `ROUND6_GATE_RESULT` 或 `ROUND6_GATE_FAILURE`。

发布记录应保存完整 stdout/stderr。任一步失败即“不具备发布证据”。单项 rerun 只用于诊断；修复后必须在最终树上从头执行组合门禁。禁止通过降低环境阈值、删除断言、改用内存替身或增加不确定 sleep 获得绿色结果。

## 9. 代码真源

- 门禁 runner：`scripts/run-multi-agent-release-gate.mjs`
- profile tests：`crates/core/src/storage/service/child_agents/tests/model_recovery.rs`、`crates/core-server/src/application/agent_dispatcher.rs`、context history tests、Renderer collaboration tests
- smoke tests：runner 内 `smokeSteps` 指向的 Rust/TypeScript 测试
- schema：`crates/core/src/storage/migrations.rs`、`canonical_schema.sql`
- Tool contract：`crates/core/src/agent_collaboration_harness.rs`
- protocol：`crates/protocol-rs/src/agent_collaboration.rs`、`packages/protocol/src/agentCollaboration.ts` 与 fixtures
- Packaged Managed Playwright assessment：`scripts/verify-packaged-playwright-startup.mjs`

## 10. 测试

文档或门禁本身变更后至少执行：

```bash
pnpm test:multi-agent-release
pnpm check
node --test scripts/reset-dev-storage-command.test.mjs
node --test scripts/verify-packaged-playwright-startup.test.mjs
```

如果只做本地快速验证，可以先运行 `--smoke-only`，但合并/发布证据必须重新运行默认组合。目标平台构建与签名验证见 [构建与发布](../development/build-and-release.md)。

## 11. 当前限制

- 门禁没有纳入 `pnpm check`，仓库也没有 CI 自动执行。
- 所有 profile 数字是确定性测试阈值，不是生产负载容量。
- 不执行真实 Provider、真实 MCP/Skill/Command/file side effect，因此不能证明第三方系统可靠性。
- 不构建、不签名、不 notarize，也不证明目标平台安装/升级。
- Packaged Managed Playwright 只覆盖 macOS unpacked startup，真实 Agent 管理浏览器链路仍 pending。
- 没有统一 release artifact manifest 把门禁日志、commit 和二进制 hash 自动绑定。

## 12. 变更检查表

- [ ] 是否仍精确运行 4 profile + 9 smoke，或已同步更新本文和 runner？
- [ ] 压力最低值是否只提高未降低，且符合产品实际配额？
- [ ] 新 fault boundary 是否加入 F1–F10 扩展清单与确定性测试？
- [ ] 六工具、RPC 和 `agent.collaboration.*` notification 是否同步 fixture/allowlist？
- [ ] schema 版本是否从 `migrations.rs` 读取并更新 reset 测试，未复制陈旧数字？
- [ ] 是否明确区分专项 gate、`pnpm check`、package/signing 和 packaged browser E2E？
- [ ] 失败后是否在最终树完整重跑，而非拼接单项结果？
- [ ] 发布证据是否记录 commit、平台/架构、工具链、命令、日志和 artifact hash？
