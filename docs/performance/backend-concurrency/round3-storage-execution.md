---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-10-18
---

# 第三轮：存储执行架构与迁移边界

起点 `f073d86d`。交付状态、最终提交与测量结果见 [PROGRESS](PROGRESS.md)。本文描述实现契约，
不把一次连接池迁移视为全部后端性能目标已经达到。

本页保留第三轮实施边界，并已加入后续 `5a31c635` 的 writer 推进修复；综合当前态见[架构契约](architecture-contracts.md)。
历史实测不重算，第四/五轮已接线的资源调度和保存所有权分别以各轮契约及当前架构文档为准。

## 依赖与磁盘连接

重新核验起点为 rusqlite 0.32.1 / libsqlite3-sys 0.30.1 / bundled SQLite 3.46.0。
本轮固定 rusqlite **0.40.2**、锁定 libsqlite3-sys **0.38.2**，启用 bundled、hooks、backup、
fallible_uint（保持旧版本的 u64/usize SQL 转换行为）。下载源码 sqlite3.h 与实际运行测试都要求
**SQLite 3.53.2**，不能只依据 crate 版本推断。

SQLite 官方确认旧版多连接 WAL 存在 WAL reset 竞态，修复包含于 3.51.3、3.50.7、3.44.6 及后续版本。
因此必须先升级依赖再启用读池。3.53.2 不是截至审阅日最新 SQLite；本轮选择包含相关 WAL 修复的
已发布兼容 Rust bundle，不声称使用最新全部修复。
依据：[官方 WAL 说明](https://sqlite.org/wal.html#walreset)、
[3.51.3 发布说明](https://sqlite.org/releaselog/3_51_3.html)、
[3.53.2 发布说明](https://sqlite.org/releaselog/3_53_2.html)、
[rusqlite 0.40.2](https://docs.rs/crate/rusqlite/0.40.2)。

磁盘库先完成原 schema migration，再验证 `journal_mode=WAL`、设置 `synchronous=FULL`、
`busy_timeout=5000ms`、`wal_autocheckpoint=1000`。读池从 **2 条** READ_ONLY / NO_MUTEX / query_only
连接起步，每条连接一次仅交给一个调用者；最多 32 个在途租借，未取得连接的等待也计数。
读取租借不拥有业务输入副本，队列探针中的 1 KiB 是租借元数据估算，不能当成查询结果大小。
`:memory:` 单元测试复用原连接，不借 shared-cache 改变数据库语义。

数据库实例身份保留在 StorageState 的同一个 Arc 中，读连接不得自行生成身份。History revision、
Trace epoch/revision、不可变发布链和失败时游标失效规则保持不变。

## 写通道、顺序与接纳

一个固定命名阻塞线程执行已迁移的完整领域操作。同步适配使用 scoped `Rayon::install` 安全借用输入，
异步入口把拥有的数据、回执与 reservation 移交 worker；没有借用 lifetime 延长、每请求新线程或默认
spawn_blocking 回退。排队使用独立 FIFO ticket，**先取得轮次再进入唯一 worker**，不依赖 Rayon 内部顺序。

2026-10-12 修正异步接纳的推进方式：首次 poll 注册拥有输入和 permit 的待执行任务；前一 ticket
释放后，直接把就绪任务提交给现有 writer，future 只等待结果。队列仅弱引用任务，取消与提交以同一
互斥槽位决定唯一归属，提交和取消析构均在锁外执行。同步后继即使占住 Tokio 调度线程，也不再需要
该线程再次 poll 异步前驱才能前进。旧实现依赖 `Notify` 唤醒后重新 poll，在同步调用挡住非可窃取的
Tokio LIFO 待运行任务时形成循环等待；已有空闲调度线程不能解除这一依赖。

`storage::state::admission_tests` 使用真实磁盘库验证：异步写入或 WAL checkpoint 首次 poll 后不再
poll，释放旧连接时仍须提交并让同步后继完成。两项测试在旧实现失败，修复后通过；另覆盖迟注册、
取消析构重入、提交/取消竞争、提交后等待者取消及 panic 后排空。早期并发基准直接构造服务，未启动
生产 bootstrap 的周期性 WAL 任务，因此不能用此前基准通过推断这一运行路径已被覆盖。
本次修复未重跑完整性能矩阵，不改变第六轮已记录的性能验收未通过结论。

初始预算：64 个已接纳操作、64 MiB 估算保留载荷，其中普通操作最多 56 个 / 56 MiB，给控制操作保留
8 个 / 8 MiB 接纳空间。控制操作仍不越过已经接纳的事务；不能以优先级中断正在提交的事务，亦不能
把容量预留写成控制延迟保证。容量依据及峰值见最终结果，不改变原 50 个逻辑运行额度或 transport lane。

[最终实测](round3-results.md) 中 writer 最大在途 20、估算字节峰值 514,750，读池最大在途 9；保留两条读连接，
未扩大预算。CPU/RSS/线程峰值及部分 timer 尾延迟回退已记录，不将容量有界等同整体资源消耗下降。

计数和估算字节包含正在执行的操作，释放后才接纳后继。大输入在排队前估算，不构造第二份完整 JSON；
Trace 热发布缓存不可变前缀大小，只扫描追加后缀。估算不是 RSS 上限，查询结果、SQLite page cache、
文件准备及上层已有输入需要分别衡量。超载返回 `storage_executor_overloaded`，超过单任务预算返回
`storage_executor_job_too_large`，关闭返回 `storage_executor_closed`，并沿原 String / StorageUnavailable
错误映射向调用者传播。接纳不等于持久化成功，现有 RPC 返回值没有提前 ACK。

同步调用依然等待真实事务结束。第二轮的 preparation / completion / control / lease 隔离仍负责这些
同步调用的异步线程边界；不能把它们全部删掉，让 Runtime emitter 在 Tokio 调度线程上等待存储。
尚未迁移的 guard 入口共享同一个 FIFO 和同一个写连接，不建立另一套数据库或隐式新写线程。
其清单与局限见 [兼容入口库存](round3-legacy-storage-inventory.md)。

## 提交可见性

update_hook 只标记三个 durable journal 的 dirty 位，不通知、不在 SQLite hook 中执行 SQL。
独占操作退出时，在 Statement / Transaction 已结束且 autocommit 恢复之后，核对真实 journal 的递增头；
只有已提交头推进才唤醒 watch。commit_hook 通过不等于提交成功；回滚、savepoint 回滚、deferred constraint
提交失败不能产生该事务的已提交通知。一个操作前半段已提交、后半段回滚时，不误删前半段的通知。

通知核对失败保留 dirty 并报告诊断，不能将已经持久化的写入改报失败。durable journal / cursor 始终是
事实来源，订阅继续初始补拉、重连补拉和原有有界轮询兜底。已有读快照即使收到 watch 也必须结束后
打开新快照，不能用旧快照误判提交尚未发生。

## 事务与权限

- Turn admission 在原完整事务内检查身份、权限、历史 revision、lease、初始 Trace、输入绑定和回执。
- 终态消息、Trace、模型上下文、usage、通知、权限撤销仍关联提交；审批身份 CAS 与通知解决不拆分。
- Mail / task / 输入 / 事件 / 幂等回执 / claim / settlement 保持现有事务边界。probe 只读，poll 会写，不能互换。
- 生产 claim 在真正获得写事务后取当前时间；跨排队的 admission / renewal 保留单调时间与最终 token 围栏。
- Skill 审批和生产 Wake transition 区分审计时间与实际期限校验；`Running → Running` 的幂等分支也不能绕过活跃 lease。队列等待和事务执行的时间都计入最终检查，失败回滚 pending / audit / Wake / usage 的全部关联事实。
- 执行权限锁仍由原调用者持有，worker 只借用只读检查闭包，事务内前后重查不删除、不提前解锁。
- 事务内不能再次调用 StorageService 取连接，也不能获取外层 admission / execution_access / credential 锁。
  持有任意连接 guard 后等待另一连接或写任务会明确拒绝，避免形成跨执行器循环。

多查询读取用短 read transaction 绑定数据和 cursor；已迁移的 history snapshot 大块解码、缓存展示复制、文件操作放到连接之外。
旧历史查询仍有持读快照解码的路径，详见兼容入口库存，后续必须继续缩短它们的快照。
Provider continuation 的 list/has replayable 包含 durable staging 恢复，因此作为完整写事务执行；
Fork 先完成恢复写入，再使用纯读计划接口，不能按函数名把这些操作归为只读。摘要只读校验省略失效派生视图，
保留严格 continuity / Provider 验证；原写路径仍负责失效清理。
旧快照不可用于执行权限的最终决定。部分恢复批处理、子树停止、World State 折叠和旧检查点解析仍是
随数据规模增长的原子操作；本轮不拆业务原子性，不宣称全部事务耗时恒定。

## 文件、Fork 与归档

Fork / child snapshot 先冻结历史及相关旁路记录，释放读取快照后克隆 Provider continuation、复制附件、
同步文件并以不覆盖方式发布；最终写事务重新检查源身份、版本、side journal、权限/模型配置、额度、
停止状态和幂等回执。side 表没有严格 revision 时用有序完整内容摘要，不能以同毫秒 updated_at 代替版本。
提交后的读回使用新快照。文件 guard 跟随真实同步提交生命周期，不随 async 等待者取消提前清理。

失败只清理本操作新建文件；提交结果不确定时保留文件供恢复核验。文件和 SQLite 没有联合 ACID，崩溃
可能留下不可达文件，但发布顺序不能让已提交引用指向尚未发布的文件。维持文件 sync_all / 目录同步；
不把这些正常故障测试等同真实断电验证。

大 Artifact 读取和 hash 在连接外完成，返回字节之前以新快照重新核验授权与完整 registry 身份。
终端历史归档的文件读取、哈希、压缩和索引准备也先在连接外完成，写通道只提交冻结的关联记录。

## WAL、备份与关闭

FULL 维持至少原同步级别，不能用 NORMAL 降低持久化换延迟。保留 1000 页自动 PASSIVE 检查点；Core Server 每
30 秒通过有界异步写入口执行 PASSIVE 并记录耗时、WAL 字节及 log/checkpointed/busy。长读快照可能钉住
WAL，PASSIVE 进展不完整不视为数据丢失；测试显式覆盖释放快照后恢复进展。

备份继续使用 SQLite Backup API、quick_check 与目标文件同步，包含尚未 checkpoint 的 WAL 提交。
不直接复制主 .sqlite 文件。关闭先停止各业务生产者及第二轮阻塞执行器，再停 checkpoint 任务、拒绝
新的连接/写接纳，等待已经接纳或已开始的操作和读取，最后关闭读池、TRUNCATE checkpoint、关闭 writer。
Provider transition 后台压缩也必须登记取消与完成归属；即使模型等待者取消，也要等待必要 receipt / usage
结算后再关闭存储，结算错误影响 shutdown 回执而不吞掉。
这类操作没有普通 Turn permit，因此在持久化 planned receipt 前同时预留 preparation / completion
数量和字节预算；同步准备在 preparation，模型等待仍异步，成功提交及取消结算在 completion。
completion reservation 跨模型等待保留，对生成结果仅原子调整同一预算，不新增无界等待队列。
调整失败保留原预算用于失败结算；结算仍无法持久化时明确记录失败，并保留原 receipt 作为恢复依据。
关机先 join 这些有归属的后台任务，再关闭阻塞通道；取消 shutdown 等待者不丢弃后台 JoinSet。
SQLite 关闭失败不得发送成功的 shutdown 回执。

未 poll 的 future 不算接纳；尚未被提交路径取得的 queued future 取消撤票，并先释放 permit 再析构
业务输入。任务被取得并进入 worker 提交路径后，等待者消失不会取消实际 SQL，预算保留至实际完成；
此处的“开始”边界是提交路径取得任务，而非 SQL 第一条指令执行。
操作遗留事务先回滚，回滚失败且事务仍开放的连接隔离，不复用给下一请求。panic、I/O 错误或不确定
提交不能推断成“必定没有变更”，恢复仍依赖原持久化 Trace、claim、receipt 和 journal。

没有 schema / 业务数据格式迁移。回退前必须正常停止生产者、完成 checkpoint/关闭并用受支持的 SQLite
备份方式保留数据；旧 SQLite 3.46.0 不能直接作为多连接 WAL 回退运行版本。任何 journal mode 切换必须在
连接全部关闭后执行，不在正在运行的用户数据库上实验。

## 第三轮结束时的后续要求

本节记录当时交给后续轮次的要求，不表示这些工作截至当前仍全部待做。第四轮已建立 Preparing/资源围栏，
第五轮已接入 Core Server 展示 checkpoint，第六轮补充 checkpoint 安装与通知响应的 lease 检查；
未全量迁移的同步桥和大型原子工作仍见当前架构契约，完整性能验收仍未通过。

第四轮建立按资源排序与 Preparing/恢复 owner 时，逐个把已经迁移的 admission、terminal、approval、
lease 服务接到拥有数据的异步写入口，再移除对应第二轮桥接等待；必须同时覆盖 queued/started/commit-unknown
取消测试。同步 Runtime emitter / Trace observer 先保留桥接，待第五轮确定持久化和事件 owner 后收口。
不得仅因 writer 已有队列就在第四轮删掉权限保护，或保留两层排队而不再跟踪。

库存中的非热路径仍共享有界 gate，后续逐项迁移，迁移须保留整个领域事务。第四轮性能验收必须继续报告
它们及历史解码、大型原子删除造成的尾延迟；此文不将它们算作已实现独立并行。

通知投递批次的 claim / validate / ack 仍保留旧 API 的 caller-shaped 时间参数，未把它们改造成 Agent 执行授权。
第五轮收敛通知持久化归属时应专门验证排队期间的投递租约与重试；不能据本轮 Agent lease 测试推断该批次协议已完成改造。
