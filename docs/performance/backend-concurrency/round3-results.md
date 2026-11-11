---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-10-11
---

# 第三轮实测结果与验收

第三轮存储执行架构已实现并完成本轮验证。起点 `f073d86d`，源码及架构契约提交
`354762d6fae3f8959027b10069a972c9b44b427f`。本页和原始证据另作本地文档提交，不推送、不发布。
第四轮可以开始；全局资源保护和部分同步桥接仍在，后端整体优化尚未完成。

## 本轮实际交付

- 实际编译 SQLite 从 3.46.0 升至 **3.53.2**，rusqlite 0.40.2 / libsqlite3-sys 0.38.2。启用多连接前核对官方 WAL reset 修复说明；磁盘测试与完整矩阵均确认实际链接版本，不只检查锁文件。
- 专属单写线程、数量及估算字节双重预算的 FIFO，另有两条只读/query_only 连接。完整领域事务在写通道中执行；控制有接纳预算预留，不中断或越过已接纳事务。
- journal 更新只标记 dirty；操作退出、事务结束后核对已提交 journal 头，才唤醒订阅。rollback、commit hook 拒绝、延迟约束提交失败和旧快照均有测试。watch 仍仅用于唤醒，durable journal/cursor 与补拉是事实来源。
- 历史快照绑定数据与游标；主要历史解码移到连接之外。Turn admission、终态、Trace、usage、审批 CAS、组织邮件/任务/输入/claim 的关联事务保持整体，数据库实例身份与 Trace 发布游标不因读池而改变。
- Fork / child snapshot 的 Provider 克隆、附件复制、hash、文件同步发布移到连接之外。登记前重查源身份、revision、旁路记录摘要、权限和 claim；失败只清理本操作文件。归档的文件读取、压缩、hash 和索引准备也移出写通道。
- WAL 使用 FULL、5 秒 busy timeout、1000 页自动检查点；增加有界周期 PASSIVE 检查点、显式关闭排空和错误传播。备份使用 SQLite Backup API，覆盖尚在 WAL 的提交。
- Provider transition 后台任务有明确 owner 和关机 join；持久化 planned receipt 前预留 preparation/completion，模型异步运行，提交与失败结算进入 completion。已启动同步 SQL 不随等待者取消而被假定停止。排队之后的 claim/审批/Wake 提交使用新鲜期限检查。

实现细节与官方依赖依据见 [存储执行契约](round3-storage-execution.md)。本轮没有修改 Electron Host、Renderer、前端数据调用、视觉、文案、布局、图标、动画、入口或业务额度、权限及审批规则，也没有新增业务 schema 迁移。

尚未迁移的同步 guard 共享同一个 FIFO 和连接，不能据此宣称所有 SQL 都在专属 writer 运行。
逐文件清单、固定字节估算的局限与第四/五轮责任见 [同步存储迁移库存](round3-legacy-storage-inventory.md)。

## 同机、同处理量的完整矩阵

比较第二轮 `d7b3fbaf` 的 `round2-full-20261011-r2` 与第三轮 `354762d6` 的
`round3-full-20261011-r1`。同一台 Apple M4、10 逻辑 CPU、16 GiB；相同 release 参数、36 个负载、
逻辑额度 50、原有 transport 容量，普通波次仍为三次、间隔 2 秒。完整运行时间 218.352 → 206.016 秒。
第三轮只有这一份完整有效矩阵，退出码 0，运行期间源码未变化；没有混入诊断采样重跑。
比较脚本拒绝负载定义、场景集合或完成计数不一致的比较，不把减少处理量当作提速。

两轮处理量完全相同：

- 1,608 RPC = 1,572 成功 + 36 个原有容量拒绝；0 无响应。50 并发审批和停止各接纳 32、拒绝 18，未扩大 lane。
- 1,420 根 Turn = 1,316 Completed + 104 Cancelled；158 子 Turn 全部 Completed。
- 50 并发短/长历史各 150 次发送全成功；60 秒固定输入稳态 620/620 成功。
- 重开后 0 InProgress；关机前 18 InProgress 与第二轮相同，来自预期审批/停止过载场景，经原关机/恢复处理。
- integrity violations、比较不一致、非预期 RPC 错误、诊断 omitted 均为 0。

队列快照在最终清理前采集，持久 Trace 终态不等于 Done 尾段或所有 reservation 已释放；
不能据矩阵尾部快照宣称所有物理队列已经 drain。实际排空、取消与关闭由专门回归验证。

响应以 **Core Server 入口计划发送时刻到成功 RPC 回执** 为边界，不含 Electron 点击到绘制。
分位数来自原始样本 nearest-rank，不平均不同场景分位数；n=1 只报告单次观察。

| 指标                             |        第二轮 |        第三轮 | 样本与解释                 |
| -------------------------------- | ------------: | ------------: | -------------------------- |
| 50 并发短历史响应 p95            |   1063.168 ms |    856.414 ms | 各 150 成功                |
| 50 并发长历史响应 p95            |   2436.857 ms |   1388.878 ms | 各 150 成功，约降低 43.0%  |
| 50 并发审批响应 p95              |    275.343 ms |    178.825 ms | 各 32 成功，约降低 35.1%   |
| 50 并发停止响应 p95              |     56.226 ms |     50.135 ms | 各 32 成功                 |
| 50 长历史准备之后的独立审批      |   2256.525 ms |   1434.417 ms | 各 1 次，仍未消除全局依赖  |
| 上一行 transport dependency 等待 |   2252.560 ms |   1431.343 ms | 各 1 次                    |
| 固定输入稳态响应 p95             |    464.998 ms |    397.067 ms | 各 620 成功                |
| 固定输入稳态完成速率             | 10.228 Turn/s | 10.248 Turn/s | 不是最大吞吐测试           |
| 50 节点单根子树 timer 最大迟滞   |      2.771 ms |      2.400 ms | 保留第二轮隔离收益         |
| 50 节点双根子树 timer 最大迟滞   |      2.727 ms |      2.329 ms | 同上                       |
| 累计已埋点 DB 等待               |      63.068 s |      34.532 s | 口径见下文                 |
| 累计连接占用                     |      44.022 s |      42.913 s | 多连接累计值，不是墙钟时间 |
| 8,830 次已埋点 commit 累计       |       4.069 s |       2.039 s | 两轮事务类别和次数相同     |
| 全矩阵 CPU user + system         |      72.114 s |      74.924 s | **增加约 3.9%**            |
| 采样 RSS 峰值                    |   276.953 MiB |   307.766 MiB | **增加约 11.1%**           |
| 采样线程峰值                     |            38 |            63 | 见独立诊断；不是精确峰值   |

没有普遍改善调度尾延迟：50 长历史 timer 最大值 **2.290 → 35.544 ms**；50 并发审批 timer
p95/最大值 **8.771/30.516 → 13.673/59.076 ms**；稳态 timer p95/最大值
**2.419/3.189 → 3.797/50.599 ms**。20 并发审批最大值 80.172 → 82.760 ms。
部分极小样本响应也回退。本轮降低了所列端到端排队与累计 DB 等待，不能声称 CPU、内存、所有尾延迟或吞吐全面优化。

### 数据库测量口径与容量依据

第二轮等待为 `sqlite.wait`；第三轮将 `storage.queue_wait`、`storage.worker_wait`、`sqlite.wait`、
`sqlite.read_wait` 相加，不重复加入覆盖整个操作的 span。worker_wait 是取得 FIFO 轮次到 worker 真正开始，
避免只报告 mutex 等待造成虚假降低。SQLite 内部 busy 等待仍落在连接占用/事务中，不能把该总数称为所有数据库阻塞时间。

第三轮 151,785 次读租借累计等连接 **1.180 s**、最大 **4.544 ms**；持连接累计 **18.755 s**、最大
**17.236 ms**。持连接包含 SQL、快照和部分未迁移解码，不是纯 SQL 执行延迟；只有第三轮有这一独立口径，
无法伪造第二轮一一对应的读池响应比较。已埋点 terminal / Trace / admission / approval 事务累计
14.976 → 13.502 秒；不含全部服务事务。对应 commit 次数分别为 1542、5626、1578、84，两轮相同；
commit 累计下降但个别 commit 最大值上升，仍需后续稳定性验证。

写通道预算 **64 操作 / 64 MiB**（普通最多 56 / 56 MiB，控制保留 8 / 8 MiB）；本次最大在途数 **20**，
最大估算保留字节 **514,750**。读池 **2 连接**、最多 **32** 租借，本次最大在途 **9**。
当前样本没有扩大容量或连接数的依据，保留这些保守起点；不把它们称为最优容量。旧 guard 的固定 16 KiB
只是兼容权重，查询结果、SQLite cache 和外层已有输入不受该数字精确覆盖，不能当作 RSS 硬上限。

### WAL、磁盘与资源边界

每场景清理后、连接关闭前的 WAL 文件样本最大 **7,041,112 bytes（约 6.715 MiB）**，第二轮为 0（非 WAL）。
36 个场景尾部文件大小之和：主库 262,217,728 → 259,747,840 bytes；WAL 0 → 158,612,912 bytes；
SHM 0 → 1,179,648 bytes。它们既不是运行时峰值，也不是累计磁盘写入量。
macOS getrusage 的 block input/output 两轮都返回 0，本轮无法可靠测出物理写入量或写放大，不能据此宣称零写入或 IO 节省。

完整性能夹具直接调用 AgentService，不经过 bootstrap 的 30 秒周期检查点，也不调用显式 `shutdown_storage`；
自动 1000 页检查点仍生效。周期管理、长快照钉 WAL、释放后进展、TRUNCATE/关闭与备份恢复由真实磁盘回归覆盖，
不能将它们的生产集成尾延迟误记为本矩阵已测量。资源样本只覆盖 Core Server 进程，不含工具子进程。

线程增加做了独立诊断：一次 50 并发审批重跑加高频 ps 与 macOS `sample`，采样会扰动耗时，**不参与性能比较**。
栈样本 55 条线程：Tokio 异步调度 10、Tokio blocking 空闲 26、固定隔离执行器 13、SQLite writer 1、
命令结算线程 1、测试/模拟服务/系统线程 4。读池本身不创建线程；稳态样本通常由第二轮 29 增至第三轮 30。
诊断高频 ps 另见峰值 80，不能用稍后的 55 条栈逐条解释 80 或正式矩阵的 63。
未发现线程泄漏证据，但保留默认 Tokio blocking pool、同步桥接和临时命令线程仍产生资源成本；
第四轮需纳入总线程预算与排队分析。本轮没有扩大 Tokio、命令线程或业务运行上限。

## 正确性验证与发现的问题

最终相同源码 release 二进制：26 组、**1,478 次通过、1,471 项不同测试、0 失败**，另有 1 项原已登记的
ignored 长历史基线。包括 Rust Core storage 1168、performance 7、Core Server 24 组共 303 次；7 个名字重复执行，
所以不把执行次数当作独立测试数。没有新增 ignored 测试。

覆盖的关键边界：

- 独立连接可见性、旧快照、回滚、savepoint、commit hook veto、提交失败重试和订阅补拉竞争。
- 单写 FIFO、数量/字节过载、控制预留、读池只读和容量、同线程嵌套拒绝、panic/遗留事务回滚、损坏连接隔离。
- async 排队取消、已开始 SQL 的等待者消失、关闭时仍在提交、关闭拒绝新接纳、长快照与检查点、包含 WAL 的备份和恢复。
- Trace 实例身份、发布不可变前缀、失败游标失效、终态/usage/上下文/通知原子性、审批身份 CAS、子树停止与幂等重放。
- Fork/child 在慢复制时释放 writer，源内容/revision/权限变化时拒绝登记，自己的 staging 清理，恢复与持久引用正确。
- 排队后 lease 过期、claim 期限起点、Skill 审批与 Wake 幂等分支的最终期限检查；拒绝旧 token 时关联写入全部回滚。
- Provider transition 的取消/失败/关机归属、同步写入不能被 abort、预算调整失败、已知 usage 结算及不能结算时保留恢复依据。
- 第二轮全部 Server 回归筛选继续执行：执行器、分派、慢准备隔离、续租、权限撤销、删除、子树、交互、审批与 terminal recovery。

开发过程中发现并修复：

1. 部分旧 release 测试未提供显式凭据后端，触发生产防护；测试注入内存凭据，双 Host 同库夹具共享该后端。没有删除生产凭据要求。
2. Provider continuation 的 list/has replayable 实际会写恢复记录，初次迁到读池导致 readonly 错误。恢复留在完整写事务，另建纯读 Fork 计划接口，不能按函数名猜只读。
3. 写队列等待暴露审批/Wake 的过期时间检查问题，增加取得事务之后和提交前的新鲜期限检查。测试曾试图缩短单调 lease 触发器，改成可控时钟推进；没有弱化数据库约束。
4. Provider transition 存在未归属后台任务及同步结算堵住 Tokio 的路径。线程采样定位后拆开准备、模型等待、提交，并在关闭前 join；测试证明 writer 被独立线程持有时 async 心跳仍能推进。
5. 一个旧 Skill worker panic 测试在看到 SQL 终态后立即断言进程 occupancy；WAL 可先观察 commit、后发生 owner 清理。改为等待对应失败 Done 后检查原有持久化/回执/occupancy 断言，不改变生产 Done 语义。

早期失败摘要与关机采样保存在归档 diagnostics，未计入最终通过数或性能样本。最终全矩阵没有减少工作量、真实落盘或诊断内容。

## 复现命令与证据

复用第二轮隔离 target，构建约 7 分 25 秒；没有碰开发进程产物。最终构建与 Clippy 命令：

```sh
CARGO_TARGET_DIR=/tmp/captain-backend-baseline-20261011-target CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4 cargo test --locked --release -p mycopilot-core -p mycopilot-core-server --lib --bin core-server --no-run
CARGO_TARGET_DIR=/tmp/captain-backend-baseline-20261011-target CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4 cargo clippy --locked --release -p mycopilot-core -p mycopilot-core-server --all-targets -- -D warnings -A clippy::collapsible_match -A clippy::useless_conversion -A clippy::unnecessary_lazy_evaluations -A deprecated
/tmp/captain-backend-baseline-20261011-target/release/deps/mycopilot_core-7748a31bd053ad1f storage:: --test-threads=4 --nocapture
/tmp/captain-backend-baseline-20261011-target/release/deps/mycopilot_core-7748a31bd053ad1f performance:: --test-threads=4 --nocapture
mkdir -p .cache/backend-concurrency/round3-validation-20261011
python3 docs/performance/backend-concurrency/results/2026-10-11-round3/regressions/run-server-regressions.py
pnpm benchmark:backend-concurrency --mode full --binary /tmp/captain-backend-baseline-20261011-target/release/deps/core_server-f7a97b10b9960ccf --output-dir .cache/backend-concurrency/round3-full-20261011-r1
python3 docs/performance/backend-concurrency/results/2026-10-11-round3/compare-results.py docs/performance/backend-concurrency/results/2026-10-11-round2 docs/performance/backend-concurrency/results/2026-10-11-round3 --json
pnpm check:docs
pnpm check:test-layout
git diff --check
```

复跑必须选择新的输出目录，并核对当前 Cargo 输出的二进制名字；不能拿本轮旧二进制为之后的代码背书。
回归脚本的源码/路径与执行日志一同保存；脚本默认写验证缓存，运行前须核对目标目录。
Rust Core storage 完成用时 225.59 秒，Core Server 最终 24 组 44.91 秒；不包含在业务性能比较里。

变更的 138 个 Rust 文件格式检查、22 项测试基础设施检查、测试布局/忽略测试登记、文档检查均通过。
Clippy 沿用前轮三类既有告警排除，另排除依赖升级后旧测试 trace API 的 deprecated 告警；没有放宽项目 lint 配置。
比较脚本额外验证了同输入、负载不一致拒绝、计数不一致拒绝、nearest-rank 规则。

[元数据](results/2026-10-11-round3/metadata.json) · [汇总](results/2026-10-11-round3/summary.json) ·
[逐场景对比](results/2026-10-11-round3/comparison-metrics.csv) · [原始 case](results/2026-10-11-round3/cases.jsonl.gz) ·
[资源样本](results/2026-10-11-round3/resources.jsonl) · [测试计数](results/2026-10-11-round3/regressions/accepted-regression-summary.json)。
完整 stage/queue 比较在 `comparison.json.gz`，所有归档文件可用同目录 `SHA256SUMS` 校验。

## 迁移、兼容、回退与下一轮门禁

1. **第四轮可开始，但不得直接删全局锁。** 仍有 admission / execution_access / RPC Write 依赖；慢准备之后审批仍约 1.43 秒。先建立按资源排序、Preparing owner、最终 CAS 与恢复协议，再拆跨对话保护。
2. 本轮迁移了核心事务，仍有 20 个服务文件、约 100 处旧 guard 入口；它们共享有界 gate，但 SQL 在调用线程、字节估算不完整。库存逐项列出了方法和责任，不能把本轮称作全部 service 已异步化。
3. 第二轮同步阻塞桥接加本轮 writer 形成两层等待。第四轮逐个将拥有数据的 admission/terminal/approval/lease 直接接 async writer，再在同一变更中验证取消/lease/权限/关机并移除对应桥接；Runtime emitter/Trace observer 的持久化 owner 在第五轮收口。
4. 部分旧历史解码仍占读快照，大型删除、恢复批处理、World State 折叠和子树停止仍随规模增长。保持事务原子性优先，本轮不能保证所有事务恒定短或所有读延迟达标。
5. 生产通知批次的投递 lease 仍用旧 caller-shaped 时间参数，第五轮须验证排队后的 claim/validate/ack；不能把本轮 Agent fencing 证明推广到不同协议。
6. Provider transition 极端输出超过全部估算预算、连失败 receipt 都无法结算时，会明确报错并保留 in_progress receipt；不能声称未知结果或已知 usage 必然已落盘。恢复依据保留，关闭不假报成功；后续需进一步完善此异常恢复策略。
7. 没有业务 schema/数据格式迁移，但连接拓扑和 journal mode 改为 WAL。回退先停生产者、排空、checkpoint/关闭，用 SQLite Backup API 保留数据；不得只复制主库，也不得把旧未修复 SQLite 直接作为多连接 WAL 运行版本。禁止在运行中的用户库实验切换模式。
8. 尚未证明可靠 p99、最大吞吐、10–30 分钟稳态、真实断电/磁盘满矩阵或 Electron 点击到 paint 的 SLO。CPU/RSS/线程和 timer 最大值的回退必须纳入后续验收，不能靠扩大额度掩盖。

本轮仅使用自动清理的临时磁盘数据库、合成历史、loopback 假模型和受控工具；没有读取/迁移真实用户库，
没有计费调用或外发消息，没有停止用户软件。保留一个约 2.6 GiB 的隔离构建目录用于后续轮次及约 1.1 MiB
压缩原始证据；没有另建大量 target，原有未跟踪缓存和截图未纳入提交或删除。
