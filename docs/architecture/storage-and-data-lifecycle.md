---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-23
---

# SQLite 存储与数据生命周期

本文说明 Rust Core 的本地 SQLite 权威存储、schema 发布策略、事务边界、文件型数据和启动恢复。本文不复制完整 DDL；`canonical_schema.sql` 及其 fingerprint 测试是唯一 schema 真源。Trace 与上下文的逻辑契约分别见[Conversation Trace 与 Exact Archive](./conversation-trace-and-archive.md)和[上下文管理](./context-management.md)。

## 职责边界

- `StorageService` 向 application 层提供领域操作；repository 负责单一表组的 SQL 与映射。
- Core Server 负责把一次业务结算组合成事务，不允许 Renderer 直接写数据库。
- SQLite 保存元数据、消息、Trace、审批、Run 恢复、设置和索引；大型附件、受管 Artifact、命令 spool 等内容保存在受管文件目录，SQLite 保存身份、hash、授权和生命周期。
- OS Keychain/受保护 credential backend 保存主密钥或 Provider secret；数据库不应保存可直接使用的明文密钥。

## 数据库定位与单实例

应用数据根目录由启动配置解析，开发/测试可使用 `MYCOPILOT_APP_DATA_ROOT`；独立存储场景可使用 `MYCOPILOT_STORAGE_DB`。生产代码应通过 bootstrap 返回的权威路径，不在各模块自行拼接默认目录。

打开数据库时启用 foreign keys、busy timeout 和 canonical schema 验证。一个进程通过 database instance lock 独占同一权威数据库；`StorageService` 当前以单个受 Mutex 保护的 `rusqlite::Connection` 串行化访问。不要绕过服务另开写连接，否则会破坏锁、事务和启动 reconciliation 假设。

## Schema 发布策略

截至本次核验，当前唯一受支持的 canonical schema 是 **v18**（SQLite `PRAGMA user_version = 18`）：

- `STORAGE_SCHEMA_VERSION = 18`；
- canonical schema fingerprint 为 `sha256:31f7d2e087d9f9bcd7b3a731c9b458fe13e9b62688bc45f7101e0733a98eb882`，由编译期常量和测试固定；
- 空数据库在一个原子流程中建立完整当前 schema；
- 非空的旧版、未知版或结构被篡改的开发数据库返回 `development_storage_schema_reset_required`；
- 当前没有受支持的原地升级链。

版本号和 fingerprint 可能变化，维护时必须读取 `crates/core/src/storage/migrations.rs`，不得从本文复制常量到运行逻辑。发布说明可以记录版本，但架构文档应强调策略而非长期维护一张迁移历史表。

开发库重置前应先关闭应用并备份数据根；删除/移动整个权威根目录后再启动，由 bootstrap 重建。不要只删除主 `.sqlite3` 而遗留 attachments、artifacts、spool 或 lock 文件。

## 领域数据地图

DDL 按领域大致分为：

| 领域                  | 代表数据                                                                                                  | 生命周期要点                                         |
| --------------------- | --------------------------------------------------------------------------------------------------------- | ---------------------------------------------------- |
| 配置与目录            | Provider/model、MCP Server registry/policy、image profile、项目、模板、偏好                               | revision/CAS；secret 与公开配置分离                  |
| 会话内容              | conversations、messages、attachments、drafts、guidance                                                    | 会话/项目归属；附件文件与行记录一致提交              |
| Agent Run             | usage、pending actions、action audit、Turn diff、world state                                              | Run/call/action identity 幂等；审批状态单向推进      |
| Trace 与上下文        | Turn Trace/items、model-context、history blob/chunk、compaction summary/head/receipt、request observation | append-only 或 immutable 派生；rewrite/fork 显式事务 |
| Provider continuation | encrypted envelopes、Tool Call bindings、transition records                                               | 私有、加密、绑定 profile/revision；promotion/release |
| Command Session       | session、output chunks、published outputs、read receipts、lifecycle                                       | 顺序 transcript；重启后保守结算/恢复                 |
| Skills                | enablement override、安装/来源相关持久快照                                                                | Skill 包内容寻址；安装发布 CAS 与 tombstone          |
| Artifacts/图像        | managed Artifacts/grants、generation execution/Artifact/config staging                                    | 内容寻址；授权与私有路径分开；启动清理               |
| 多 Agent              | nodes、mailbox、wake/interrupt、delivery receipt/replay、context snapshot、collaboration event            | 图和队列限制在事务内复核；cursor/receipt 幂等        |
| 搜索索引              | message/archive FTS 等                                                                                    | 可重建，不是权威内容                                 |

新增表前先确定领域 owner、父对象、删除策略、敏感级别、幂等键和恢复行为。不要把跨领域工作流塞进单个 repository。

## 事务原则

1. **外部调用不占写事务。** 模型请求、命令执行、网络下载和摘要生成在事务外完成；提交时重新验证 revision/归属。
2. **先持久授权，再开始副作用。** pending action 的批准与 audit/CAS 在事务中完成，事务提交后才执行 Core Server-owned 动作。
3. **终态一起提交。** assistant message、Trace/model-context 终态、Usage 和相关 projection 在同一业务边界写入，避免 UI 成功而历史未结算。
4. **Immutable + head。** Compaction summary、Archive blob、Artifact 内容等不可变对象与当前 head/grant 分离。
5. **幂等 identity。** `request_id`、`run_id`、`call_id`、`action_id`、execution fingerprint 等必须在数据库约束/CAS 下判定，不靠内存去重。
6. **文件采用 staging + fsync/原子发布。** 数据库记录和最终文件路径必须有明确提交顺序及启动清理策略。
7. **调用方事务。** 标注 `*_in_transaction` 的 repository 函数不自行 begin/commit；事务所有权属于组合业务操作。

## 启动与崩溃恢复

Bootstrap 大致执行：解析数据根与锁、打开/校验 canonical schema v18、构造 repositories/services、加载 MCP Server/Provider/Skills/Artifact Runtime、随后运行领域 reconciliation。

恢复必须按“数据库已提交状态”判断，不按 Renderer 缓存判断。当前需要关注：

- orphaned `in_progress` Trace 和未终结 assistant Run；
- interrupted pending action、approval successor 和 action audit；
- Provider continuation prepared/promoted/released 状态；
- Command Session 进程已消失、transcript/receipt 尚未结算；
- image generation/admitted execution 的 succeeded、failed 或 indeterminate；
- Artifact staging、临时文件、过期 grant/安装 session；
- collaboration delivery、wake/interrupt 和子 Agent snapshot receipt。

当外部副作用可能已发生但数据库没有成功确认时，恢复结果必须使用 `outcome_indeterminate`/`commit_indeterminate` 等保守状态，不能自动重放付费生成、写文件或命令。

## 敏感数据与文件生命周期

- API key、Provider credential、continuation 加密主密钥不进入日志、Trace 或普通配置行。
- Provider continuation payload 加密后存储，元数据仍必须最小化并绑定归属。
- Attachment、Artifact、Skill package 和 command workspace 通过服务解析 URI/ID；数据库路径不是 Renderer/模型 API。
- Artifact 使用内容 hash 标识与 conversation grant 分离；删除会话时撤销授权，并按引用/保留策略清理物理内容。
- Exact Archive 只保存安全文本投影；二进制和 data URL 被省略并记录诊断。
- action audit 可能有意晚于消息内容存活；删除策略必须由 service 显式实现，不能假设所有外键都 cascade。

## 备份、恢复与可观测性

当前开发策略以整套数据根备份/重置为主。复制在线 SQLite 文件并不等价于一致备份；应在应用关闭、锁释放后复制数据库及其配套文件目录，或使用受支持的 SQLite snapshot/backup 流程。

错误日志可包含 schema version、fingerprint、record identity、phase 和 recovery code，但不得打印消息正文、原始 Tool 参数、continuation 密文解密结果、密钥或私有绝对路径。

## 不变量

1. `canonical_schema.sql`、canonical schema v18 的 version 与 fingerprint 必须一致。
2. 非空非当前 schema fail closed，不自动执行未审计迁移。
3. 所有领域对象在 service SQL 边界校验 conversation/project/Run 归属。
4. 外部副作用与数据库提交之间的崩溃窗口必须有明确恢复状态。
5. FTS、Renderer JSON 和缓存均不是权威数据。
6. 内容寻址对象的 hash 针对发布后不可变字节；mutable head/grant 单独更新。
7. 删除、rewrite、fork 和 startup reconciliation 必须保持跨表/文件引用完整。

## 代码真源

- Schema/version：`crates/core/src/storage/canonical_schema.sql`、`migrations.rs`
- 服务与连接：`crates/core/src/storage/service.rs`、`storage/mod.rs`
- 单实例：`crates/core/src/storage/database_instance_lock.rs`
- 快照：`crates/core/src/storage/database_snapshot.rs`
- Repositories：`crates/core/src/storage/*_repository.rs`
- 领域组合事务：`crates/core/src/storage/service/`
- Bootstrap：`crates/core-server/src/transport/bootstrap.rs`

## 测试

- `crates/core/src/storage/migrations.rs` 内 canonical schema/fingerprint 测试
- `crates/core/src/storage/*_repository.rs` 及其 `tests/`
- `crates/core/src/storage/service/tests/`
- `crates/core/src/storage/service/tests/reconciliation.rs`
- `crates/core/src/storage/service/tests/trace_reconciliation.rs`
- `crates/core/src/storage/agent_command_session_repository/tests.rs`
- Core Server 的 pending action、Provider transition、Command Session、image generation 和 collaboration 测试

## 变更检查表

- [ ] 修改 `canonical_schema.sql` 后同步 canonical version、fingerprint 和 fresh-schema 测试；若版本不再是 v18，同时更新本文当前快照。
- [ ] 明确旧数据库行为；没有经批准的迁移链时保持 reset-required。
- [ ] 新表/列定义 owner、FK、唯一键、索引、删除/保留和敏感分类。
- [ ] 跨表操作在一个 service 事务中完成，并有冲突/幂等测试。
- [ ] 外部工作在事务外执行，提交时重新校验 revision/CAS。
- [ ] 新文件目录采用受管根、staging、原子发布与 orphan cleanup。
- [ ] startup reconciliation 覆盖进程在每个提交边界崩溃的状态。
- [ ] fork/rewrite/delete/backup 行为同步更新相关领域文档。

## 当前限制

- 当前没有面向用户数据的原地 schema 升级承诺，旧开发库需要整根重置。
- 单连接 Mutex 设计偏向桌面本地一致性，不适合多进程或高并发服务端部署。
- 自动保留期限、跨设备同步、在线增量备份和用户级导出策略尚未形成统一公共契约。
- 物理文件和 SQLite 无法共享单个 ACID 事务，依赖 staging、原子改名和 reconciliation 收敛。
- SQLite FTS/索引损坏时需要重建；索引不可替代原始消息或 Archive。
