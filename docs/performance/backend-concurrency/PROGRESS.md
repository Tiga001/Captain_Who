---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-10-11
---

# 后端并发优化交付记录

## 第三轮：存储执行架构

状态：**第三轮实现及本轮验收完成，第四轮可以开始；后端整体优化尚未完成。**

起点 `f073d86d`，源码终点 `354762d6fae3f8959027b10069a972c9b44b427f`（`refactor(storage): isolate bounded WAL writes and snapshot reads`）。
本页与最终证据另作本地文档提交；可用 `git log -1 -- docs/performance/backend-concurrency/PROGRESS.md` 查询最终交付提交。
未推送、未发布；未改 Host、Renderer、前端数据逻辑、视觉或业务额度、权限和审批规则。

已交付：

- 实际 SQLite 3.53.2，包含官方 WAL reset 并发修复；rusqlite 0.40.2 / libsqlite3-sys 0.38.2。FULL、busy timeout、自动及周期检查点、Backup API 和显式关闭管理。
- 单专属 writer，有数量/估算字节限制的 FIFO 和控制接纳预留；两条只读连接与有界租借。完整 admission/terminal/Trace/审批/邮件任务事务保持原子性。
- journal hook 只记录 dirty；成功提交且已提交 journal 头推进后再通知，rollback/commit 失败不提前唤醒。读快照与游标一致，Trace 实例身份和发布失败游标规则保持。
- Fork/child snapshot 的文件复制、Provider 克隆及 hash 移出连接，发布后在完整短事务中重新检查源与权限；历史主要解码、Artifact 读取和归档准备移出连接。
- 补齐写队列后的 lease 新鲜检查，Provider transition 的任务归属、有界准备/提交、取消结算与关机等待；已开始同步 SQL 不随 async 等待者消失而丢弃。

验证与收益：

- 最终 1,471 项不同 Rust release 测试、1,478 次执行全部通过，1 项原有 ignored；138 个变更 Rust 文件格式、Clippy（既有三类排除及旧 trace API deprecated 排除）、文档与测试基础设施检查通过。
- 36/36 同机同负载场景通过；1,608 RPC、1,420 根 Turn、158 子 Turn 的成功/拒绝/终态分布与第二轮完全一致，重开后 0 InProgress。
- 50 并发长历史发送 p95 **2436.857 → 1388.878 ms**，审批 **275.343 → 178.825 ms**，停止 **56.226 → 50.135 ms**；已埋点累计 DB 等待 **63.068 → 34.532 s**。
- 固定输入稳态 620/620 成功，响应 p95 **464.998 → 397.067 ms**，完成速率约 10.25 Turn/s；这不是最大吞吐证明。
- **代价与残余：** CPU +3.9%，采样 RSS +11.1%，线程峰值 38 → 63；若干 timer 尾延迟回退。慢准备之后审批仍约 1.43 秒，全局依赖尚未移除。磁盘物理写入量不可可靠测得，WAL 文件尾部大小不能当成写入量。

[存储执行契约](round3-storage-execution.md) · [完整实测与限制](round3-results.md) ·
[剩余同步入口库存](round3-legacy-storage-inventory.md) · [原始证据](results/2026-10-11-round3/summary.json)

第四轮前置条件已满足：提交后通知、完整事务写通道、共享身份的只读池、WAL/恢复回归已具备。
第四轮先完成资源归属、Preparing/最终 CAS 与恢复协议，再拆 admission / execution_access / RPC 跨对话保护。
第二轮桥接与本轮 writer 的重复等待必须逐入口移除；库存中的 20 个服务文件、约 100 处旧 guard 仍共享有界 gate，但不能算作全部异步 writer 已迁移。
同步 Trace observer/Runtime emitter 及通知批次投递归属在第五轮收口。默认 blocking pool 与临时命令线程总预算、资源峰值和长快照也列入后续验收。

本轮只使用临时磁盘数据库、合成历史、loopback 假模型和受控工具；未触碰用户库、计费服务或正在运行的软件。
复用同一个约 2.6 GiB 隔离 target，夹具自动清理，保留约 1.1 MiB 压缩原始证据。原有未跟踪缓存和截图未提交或删除。

## 第二轮：阻塞隔离与控制响应

状态：**第二轮实现及本轮验收完成，第三轮可以开始；后端整体优化尚未完成。**

起点 `6c421d07`，源码终点 `d7b3fbaf`。本地提交：

- `b1737f51`：Wake admission、续租、等待批准及终态事务使用新鲜期限与冻结身份 fencing，失效 token 不写入继任者状态。
- `46b35ea0`：有界准备、维护、续租、控制、终态执行通道；Control / Approval 专属物理执行；取消/关机接管与竞态回归。
- `d7b3fbaf`：持久终态后释放精确旧 owner，单 worker 回执恢复保留原 admission 保护并移除轮询重入，Done 仍等待必要尾段。

记录与原始证据另作本地文档提交；最终交付提交可用 `git log -1 -- docs/performance/backend-concurrency/PROGRESS.md` 查询。未推送、未发布，未改 Host、Renderer 或前端数据逻辑；视觉、业务运行额度和权限/审批规则保持不变。

本轮结果：

- 36/36 同机同负载场景通过。1,608 RPC = 1,572 成功 + 36 原有容量拒绝；1,420 根 Turn、158 子 Turn 持久终态与基线完全一致，重开后 0 InProgress。
- 50 节点单根子树最大调度迟滞 691.319 → 2.771 ms；双根 427.771 → 2.727 ms。50 并发审批/停止成功响应 p95 为 633.473 → 275.343 ms、269.299 → 56.226 ms。
- 60 秒固定负载 620 次全成功，完成速率约 10.23 Turn/s，响应 p95 489.077 → 464.998 ms；不宣称最大吞吐提升。
- 50 长历史发送 p95 上升约 9%，全矩阵 CPU +7.1%，线程采样峰值 26 → 38。慢准备之后的审批仍约 2.26 秒，主要来自保留的全局写依赖。没有把线程隔离等同端到端性能达标。
- 最终 221 项不同 Rust release 回归全部通过（21 组、223 次执行）；另有中断回执恢复 20 次重复。变更 Rust 格式、Clippy（沿用第一轮三类既有排除）、文档及测试布局检查均通过。

[执行器生命周期、容量与锁顺序](round2-blocking-lifecycle.md) · [完整实测、失败试跑、命令及局限](round2-results.md) · [原始证据](results/2026-10-11-round2/summary.json)

第三轮前置条件已满足：已有可复现基线、有界阻塞桥接和 lease/terminal fencing。下一轮先完善 after-commit 通知与一致快照，再迁移短事务单写通道和小读取池，逐入口删除本轮临时的存储双重等待。同步 Runtime emitter / Trace observer、单 SQLite 连接竞争仍需处理。
第四轮之前继续保留 admission / execution_access / RPC 全局顺序；完成 Preparing、最终 CAS、资源归属与恢复协议后再拆除。不得靠增加额度或跳过持久化消除本轮残余等待。

本轮只使用隔离临时磁盘库、loopback 假模型和受控工具，未触碰用户数据库、计费模型或运行中的软件。复用同一个隔离 target；合成夹具由测试自动清理，保留约 1.4 MiB 原始证据与有限构建缓存供下一轮使用。原有未跟踪缓存和截图不纳入提交。

## 第一轮：性能基线与并发契约

状态：**第一轮完成，第二轮前置条件已具备；后端整体优化尚未完成。**

起点：`aac5fa6b`，包含独立审批分派、审批身份索引和 World State 复用。源码终点：`acae0662`，最终有效基准在此提交执行；本页及原始结果作为独立文档提交交付。本轮只增加可关闭的测量、真实调用链基准和契约，不提高业务额度，不改变事务和现有 RPC 成功语义。原有未跟踪缓存及截图不属于本轮交付。

本地实现提交：

- `374e529e`：Core Server / SQLite / Host / Renderer 分段诊断、真实磁盘夹具、运行与校验脚本、行为回归。
- `32aa536e`：固定合成模型上下文，阻止自动压缩污染并发基线。
- `acae0662`：稳态夹具通过真实接口续租，保留生产 60 秒授权期限。

不推送、不发布。查阅包含本页的文档交付提交可运行 `git log -1 -- docs/performance/backend-concurrency/PROGRESS.md`。

配套资料：

- [测量结果与局限](results.md)
- [资源、并发、事务与恢复契约](architecture-contracts.md)
- [验收基线与竞态测试映射](acceptance.md)
- [Host / Renderer 测量入口和口径](host-renderer-measurement.md)

### 已落地的测量能力

1. Core Server 固定桶 p50/p95/p99 上界、样本数、最大值、累计时间；静态标签最多 256 组，超出显式计数。最多 32 个数值 RPC ID trace / 窗口，不记录正文、密钥或真实对话 ID。
2. 真实分派链的 admission、lane、依赖围栏、执行许可、blocking pool 和 handler 执行分段；队列 count/bytes/oldest/peak；既有队列容量、顺序和全局围栏保持不变。
3. Turn admission/access 锁等待与持有、历史准备、持久化接纳、runtime 首次调度；SQLite 连接等待/持有、按静态调用族分类、领域事务及 commit。
4. Renderer 前置保存/RPC/应用跨度、Host 序列化/write 回调/response 和事件循环延迟；独立 IPC 诊断参数关联 Renderer action → Host RPC ID → Core Server RPC ID。默认关闭时保留 Promise、异常和原业务调用参数路径。
5. 实际 `RpcRequestDispatcher`、`handle_request`、`AgentService`、磁盘 SQLite、loopback 假模型和真实安全 shell 工具的测试夹具，包含 1/5/10/20/50 单元、长短历史、审批、停止、单根/多根子 Agent、慢工具和固定负载稳态。
6. 独立 release 构建/采样脚本，保存源码/差异/文件清单/锁文件/测试二进制 SHA-256、机器、编译器、运行参数、逐请求原始数据、进程资源和 SQLite 信息。源码在测试期间发生变化会将运行标为无效。

### 复现入口

```sh
pnpm benchmark:backend-concurrency --mode smoke
pnpm benchmark:backend-concurrency --mode full
```

默认复用 `/tmp/captain-backend-baseline-20261011-target`，release、debug info 0、incremental 0、编译 jobs 4；输出到新的 `.cache/backend-concurrency/` 子目录。已有输出不会覆盖。
可用 `--output-dir` 指定新目录、`--filter` 选择场景、`--waves` 控制普通输出轮数；`--binary` 仅用于已经核验源码一致的测试二进制。普通输出每 2 秒一批，稳态独立固定为 31 批 / 60 秒。容量仍为 50，approval/control lane 仍为 32；过载拒绝必须纳入数据。

### 兼容性边界

- 不改布局、CSS、图标、动画、文案或交互入口；Host/Renderer 仅增加有界诊断及可选诊断 metadata。
- 不改数据库 schema、journal/synchronous 配置、权限/审批/取消/归档/删除规则，不拆事务，不提前 ACK。
- 不访问用户数据、不调用外部模型、不发送真实消息、不停止现有软件。临时数据库由 fixture 自动清理。
- 既有 history 缓存诊断只在 debug 依赖中提供；release 测试仅条件编译其计数断言，保留功能与 Arc 复用断言，不为压测打开 release debug assertions。
- 相关磁盘恢复夹具显式注入内存凭据后端，避免依赖开发模式默认行为。假模型使用 `mycopilot-core/test-support` 特性允许受控 loopback HTTP；仅 Core Server 的 dev-dependency 开启，正常生产依赖图仍要求 HTTPS。该特性也拒绝远程/内网非回环 HTTP，不改变真实产品的连接策略。

### 第二至六轮依赖顺序

第二轮：可开始。先根据队列/准备/数据库分段数据收敛阻塞执行与控制路径，补齐子节点准备分段。50 单元单根子树 timer 最大迟滞 691.319ms，50 单元混合准备的一次审批依赖等待 2096.149ms 是具体复现入口；保留权限 epoch、准备最终校验、租约和恢复之间的关系。不能用扩大队列代替消除阻塞。

第三轮：先修复提交后通知与完整领域事务，再引入短事务单写通道/小型读取池。核验 SQLite 版本与 WAL 适配后再迁移，不能直接换连接池导致未提交通知或历史快照不一致。

第四轮：在资源归属、Preparing/最终 CAS 和恢复语义落实后拆全局串行；普通不同 Conversation 独立，同 Conversation/AgentTree/删除/Account epoch 保留明确顺序。50 对话短/长历史接纳回执 p95 为 1002.674/2235.920ms，而 handler p95 桶上界为 36.863/65.535ms，优先消除跨对话排队，不能直接放开存在真实原子性职责的锁。

第五轮：基于 durable journal/cursor 收敛 Core Server 持久化与前端保存责任；不删除尚未有 durable owner 的保存，不吞 Trace 或错误。

第六轮：同源码/负载/机器下做完整对比和故障/长稳态验收。第一轮的低样本矩阵不能替代最终 p99/SLO 证明。

### 当前验证状态

最终有效矩阵：`round1-full-20261011-r3`，release、36/36 场景、退出码 0、源码未变化、完整性校验无错误。1,608 个 RPC = 1,572 成功 + 36 预期过载拒绝；1,420 根 Turn 和 158 子 Turn 的持久终态逐项核对。620 次固定负载稳态发送全部成功。前三瓶颈、全部原始资料和明确的测量范围见 [results.md](results.md)。先前两次失败的完整矩阵只用于修正夹具，不参与最终比较。

验证通过：

- 定向 Rust release 回归 86 项；夹具最终修正后另重跑 3 项载荷/统计/拒绝分类测试和完整矩阵。
- Host/Renderer：7 个 unit 文件共 87 项、2 个 browser 文件共 15 项，node/web 类型检查，本轮 TS ESLint。
- 测试基础设施 22 项、忽略测试/测试归属检查、文档检查、格式及 `git diff --check`。
- 严格 Clippy 被 3 个既有告警阻挡；只在检查命令排除对应 3 类后，两个 crate 全部 targets 通过。告警位置、命令和限制见结果页；没有放宽项目 lint 配置。

尚未证明：原生点击到 React paint 的端到端 SLO、持续 token 流/慢消费者、诊断开销 A/B、可靠 p99、最大吞吐、10–30 分钟稳态，以及新架构所需的进程崩溃/磁盘满/租约/准备删除完整故障矩阵。这些是后续轮次的门禁，不能用本轮通过替代。

资源清理：临时磁盘夹具正常自动清理；首次中断残留的合成数据库已核对名称、创建时间和无打开句柄后删除。保留同一个约 2.0 GiB 的隔离 target 供后五轮复用，未新增多个构建缓存；真实开发 target、用户数据库、原有缓存和截图均未清理。原始有效证据归档约 1 MiB。
