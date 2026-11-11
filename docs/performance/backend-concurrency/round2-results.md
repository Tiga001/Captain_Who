---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-10-11
---

# 第二轮实测结果与验收

本轮是阻塞隔离与控制响应，不是全局锁拆分或存储池迁移。起点 `6c421d07`，源码终点 `d7b3fbaf`。
本地提交为 `b1737f51`（存储 fencing）、`46b35ea0`（有界执行与控制隔离）、`d7b3fbaf`（终态 owner 退休与有界回执恢复）。本页及证据另作文档提交，不推送、不发布。

## 实现范围

- 同步子任务准备、运行时 Host 组装、终态结算、调度器维护和续租分别进入固定大小、有数量及估算字节预算的执行资源。模型和异步工具继续由 Tokio 运行。
- RPC Control / Approval 保留原接纳容量、执行数、排序及返回含义，改为独立物理 worker；默认 blocking pool 饱和时仍有执行能力。
- claim 前先预留准备容量；claim 后独立续租。准备结束、开始执行及终态事务重查冻结 token，过期或换代不能覆盖继任者。同步事务等待连接之后及提交前使用新鲜时间，不延长 TTL。
- 取消区分排队与已经开始的同步变更；关闭排空已提交工作，拒绝晚提交。异常调度器退出也接管已经启动的准备。关机后已经持久接受的回答留待恢复，不再启动无人管理的新模型段。
- 终态和后续交互投递预算一次性预留，Done 保持在持久化与必要投递之后，避免旧协调器跨越 Done / 重启。Trusted 终态用量局部累计，旧 token 拒绝不回滚接手者的共享状态。
- 原子终态提交后及时释放精确旧 owner。需要 admission 的回执恢复由单独一个有界 worker 等待，去掉每 Turn 轮询重入；等待批准、等待交互和持久失败等清理不排在这个锁之后。

未修改 Host、Renderer、前端数据调用、视觉设计、业务运行额度、模型/工具能力、schema 或 SQLite journal/synchronous 配置。等待批准与等待交互的额度规则沿用原实现。
执行器容量、锁顺序、租约、关闭协议及第三轮清理责任见 [生命周期说明](round2-blocking-lifecycle.md)。

## 最终有效矩阵

第一轮 `acae0662` 的 `round1-full-20261011-r3` 对比第二轮 `d7b3fbaf` 的 `round2-full-20261011-r2`。
同一台 Apple M4 / 10 逻辑 CPU / 16 GiB，release、相同 36 个负载、相同 50 逻辑运行额度及 transport 接纳容量，普通波次仍为 3 次、间隔 2 秒。第二轮完整运行 218.352 秒，第一轮 213.760 秒。
源码指纹在运行期间未变化，退出码 0，完整性错误、诊断 omitted、非预期 RPC 错误均为 0。
运行时源码与 HEAD 一致；尚未提交的文档索引差异已纳入原始元数据的 tracked diff 指纹。

逐项复核实际处理量，两轮完全相同：

- 1,608 个 RPC：1,572 成功、36 个既有容量拒绝、0 个无响应。36 次拒绝来自 50 并发审批和停止突发各 18 个，原 lane 容量 32，未扩大容量。
- 1,420 个根 Turn：1,316 Completed、104 Cancelled；158 个子 Turn 全部 Completed。重开磁盘数据库后 0 个 InProgress。
- 50 并发短、长历史各 150 次发送全部成功；固定负载稳态 620 次发送全部成功。
- 关机前仍有 18 个 InProgress，与基线相同，来自预期审批/停止过载夹具；经原关机/重启恢复后为 0。不能把关机前全部 drain 当作已证明。

队列快照采集在最终清理/关机之前；其中 completion count 包含尚未使用的尾段 reservation，少数场景在 Trace 已终态后仍有尾段预算。矩阵的 `drained_before_cleanup` 判断是逻辑额度与持久 Trace，不是所有执行器/Done 尾段排空。执行器真实排空、晚提交与 Done 次序由相应回归单独验证，不能把这张快照当作零泄漏证明。

以下响应从 **Core Server 入口的计划发送时刻** 算起，仅统计成功 RPC；不是 Electron 点击到界面绘制耗时。分位数为原始样本 nearest-rank，n=1 仅列单次观察，不能解释为 p95。

| 指标 / 场景                         |        第一轮 |        第二轮 | 解释                               |
| ----------------------------------- | ------------: | ------------: | ---------------------------------- |
| 50 节点单根子树 timer 最大迟滞      |    691.319 ms |      2.771 ms | 已隔离子节点同步准备               |
| 同场景 timer p95                    |    631.319 ms |      2.522 ms | 78 / 91 个 timer 样本              |
| 50 节点双根子树 timer 最大迟滞      |    427.771 ms |      2.727 ms | 76 / 94 个 timer 样本              |
| 50 并发审批成功响应 p95             |    633.473 ms |    275.343 ms | 各 32 个成功，拒绝数不变           |
| 50 并发停止成功响应 p95             |    269.299 ms |     56.226 ms | 各 32 个成功，拒绝数不变           |
| 50 长历史准备之后的独立审批（单次） |  2,102.434 ms |  2,256.525 ms | 全局写依赖仍阻塞，未解决           |
| 上一行的 transport dependency 等待  |  2,096.149 ms |  2,252.560 ms | 第四轮资源归属协议前保留           |
| 50 并发短历史发送 p95               |  1,002.674 ms |  1,063.168 ms | 各 150 次成功，约 +6.0%            |
| 50 并发长历史发送 p95               |  2,235.920 ms |  2,436.857 ms | 各 150 次成功，约 +9.0%            |
| 60 秒固定负载发送 p95               |    489.077 ms |    464.998 ms | 各 620 次成功                      |
| 同稳态完成速率                      | 10.231 Turn/s | 10.228 Turn/s | 受固定输入速率限制，不是最大吞吐   |
| 同稳态最大响应                      |  1,331.879 ms |    583.099 ms | 仍需更长稳态复测                   |
| 全矩阵累计 CPU user + system        |      67.348 s |      72.114 s | +7.1%，没有全链路 CPU 提速         |
| 采样进程 RSS 峰值                   |   290.953 MiB |   276.953 MiB | 非精确峰值，也非堆硬上限           |
| 采样进程线程峰值                    |            26 |            38 | 固定物理隔离成本；不是业务额度增长 |

1 / 10 / 30 秒慢工具期间，另外四次独立审批的最大响应为 8.498 / 9.223 / 10.209 ms；基线为 8.091 / 7.508 / 11.394 ms。
这些小样本只证明受控慢工具不会占住审批所需物理资源，不能推广为任意存储停顿时的控制 SLO。
子树准备的调度迟滞显著降低，但子树完整处理耗时和普通发送没有普遍提速。新增 fencing、线程切换和保留的单连接/全局锁都有成本；第三、四轮仍是降低端到端排队的必要工作。

原始 [元数据](results/2026-10-11-round2/metadata.json)、[汇总](results/2026-10-11-round2/summary.json)、[逐场景对比](results/2026-10-11-round2/comparison-metrics.csv)、[cases](results/2026-10-11-round2/cases.jsonl.gz)、[进程资源](results/2026-10-11-round2/resources.jsonl) 和全部回归日志已归档。
`comparison.json.gz` 保留完整分段对照；`SHA256SUMS` 校验归档文件。

## 试跑发现并修复的回退

首次全矩阵 `round2-full-20261011-r1` 不合格：50 长历史仅 102/150 成功，已落盘终态仍被全局恢复锁拖住进程内 occupancy，使 48 次后续发送误报 active run。
只提前释放 durable owner 后的定向复测也不合格：120/150 成功，30 次过载；每 Turn 每 10ms 尝试同一把 admission 锁，使 completion 作业由正常 150 次放大到 19,084 次。
这次 runner 虽退出 0，实际成功数低于基线，因此没有当作通过。

最终实现保留全局恢复保护，改由一个固定 recovery worker 阻塞等待，并在原子终态提交后注销旧 owner。
原逻辑额度、队列容量、波次和处理内容均未扩大或减少。定向复测为 150/150 成功、150 个 Completed、150 次 completion、150 次 recovery；完整矩阵再次确认相同处理量。
失败与通过定向样本的元数据、长历史 case 和日志保留在归档的 `attempts/` 中，不混入最终性能统计。

## 验证方法

使用第一轮相同隔离目录及 release 参数，未操作真实用户数据库或正在运行的软件。所有模型为 loopback 假服务，工具限于临时目录中的受控命令。

```sh
CARGO_TARGET_DIR=/tmp/captain-backend-baseline-20261011-target CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4 cargo test --locked --release -p mycopilot-core -p mycopilot-core-server --lib --bin core-server --no-run
CARGO_TARGET_DIR=/tmp/captain-backend-baseline-20261011-target CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4 cargo clippy --locked --release -p mycopilot-core -p mycopilot-core-server --all-targets -- -D warnings -A clippy::collapsible_match -A clippy::useless_conversion -A clippy::unnecessary_lazy_evaluations
python3 docs/performance/backend-concurrency/results/2026-10-11-round2/regressions/run-regressions.py
pnpm benchmark:backend-concurrency --mode full --binary /tmp/captain-backend-baseline-20261011-target/release/deps/core_server-a5ca48c145d29d84 --output-dir .cache/backend-concurrency/round2-full-20261011-r2
python3 docs/performance/backend-concurrency/results/2026-10-11-round2/compare-results.py docs/performance/backend-concurrency/results/2026-10-11 docs/performance/backend-concurrency/results/2026-10-11-round2 --json
pnpm check:docs
pnpm check:test-layout
git diff --check
```

复跑必须使用新的基准输出目录；二进制文件名与当前 Cargo 构建输出核对。回归脚本记录当时的二进制名，输出到本轮验证缓存目录，不能不经构建直接当作新源码的证据。

最终 release 回归为 21 组、223 次执行、221 项不同测试，0 失败（settlement clock 两项同时包含在 agent_graph 总组内）。覆盖：

- 执行器数量/字节过载、显式排队取消、丢弃 future 后仍真实完成、panic、reservation 配对和关闭/晚提交。
- 慢准备时控制及续租独立推进；准备容量用尽不提前 claim；准备中取消、树停止、正常和异常 manager 关机。
- 排队/host 准备后的过期 token、权限撤销、generation 变化与删除；可信终态 fence 拒绝不写消息/用量、不覆盖新 owner。
- 外部 SQLite writer/连接等待期间租约过期，终态事务回滚与精确幂等重放；重开磁盘数据库恢复。
- admission 被占时终态落盘、精确 owner 清理及 completion 槽位可用；占满剩余 completion 预算后 recovery 仍能完成且 Done 不提前。
- 同步/异步交互、审批顺序、子树协作、文件效果及删除超时、关机后已持久回答重启恰好一次接续。

既有中断回执恢复测试修正夹具后另重复 20 次，全部通过；见归档回归记录。变更的 48 个 Rust 文件格式检查通过。

Clippy 命令仅沿用第一轮三个既有告警类别的排除；未放宽项目 lint 配置。执行器显式排队取消 API 目前用于验证，生产 claim owner 必须 drain，因此该 API 的非测试 dead-code 使用局部 `expect` 标明。
全仓 `cargo fmt --all -- --check` 仍指出原有 `transport/mod.rs` 模块排序；本轮不夹带该文件，单独检查全部变更 Rust 文件。
部分既有集成夹具在 release 下默认没有凭据后端，本轮显式注入内存凭据；不改变生产凭据策略。早期验证发现的异步回答路由/Done 先后问题已通过恢复原同步路由和等待投递尾段修正，失败试跑不计入最终证据。
另有既有恢复测试持续同步查询 SQLite，导致新 maintenance worker 写回执饥饿：失败进程采样 716/719 次停在连接锁。
夹具已改成等待调度扫描通知后单次核对持久回执，保留原 1 秒截止及恰好一次中断断言；没有延长超时或修改生产锁。
这也说明单连接互斥的公平性问题仍真实存在，属于第三轮迁移范围。

## 必须保留的局限

1. 单 SQLite 连接及同步 Runtime emitter / Trace observer 回调仍在；长连接持有仍可能延误控制和续租。本轮失效后拒绝旧 owner 提交，不承诺任意 IO 停顿下租约永不丢失。
2. 全局 admission、execution_access、RPC Write 及依赖围栏保留；控制 worker 可运行不等于绕过全局依赖，第四轮必须先完善资源归属与恢复协议再拆除。
3. 执行器字节预算是估算权重，未精确包含全部历史、输出增长、共享 Arc 和数据库 cache，不是 RSS 硬上限。没有增加业务运行额度。
4. 当前桥接池不是最终存储架构。第三轮迁移 writer / 读池时应逐入口删除重复等待，禁止永久叠加另一层存储排队；先完善 after-commit 通知与一致快照。
5. 受控单线程测试证明执行机会，短矩阵给出同机样本；不证明可靠 p99、最大吞吐、10–30 分钟稳态、磁盘满、断电恢复或 Electron 点击到 paint 的端到端 SLO。
