---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-10-18
---

# 后端并发优化交付记录

## 2026-10-18：当前源码文档核验

当前源码基线为 `5a31c635`。本次仅将[系统总览](../../architecture/overview.md)、[Core Server](../../architecture/core-server.md)、[存储生命周期](../../architecture/storage-and-data-lifecycle.md)、[Runtime](../../architecture/agent-runtime-and-providers.md)、[Multi-Agent](../../architecture/multi-agent.md)及恢复/发布文档与实现对齐：WAL/FULL 单 writer/双 reader、有界阻塞与资源调度、Core Server checkpoint owner、lease/final CAS、关闭顺序及精确 v71 迁移边界。没有改写各轮测量数据或原始证据，也没有重跑完整性能矩阵。后续修复与第六轮失败门禁继续分别记录；当前实现差异汇总见[架构契约](architecture-contracts.md)。

## 2026-10-12：运行卡死与切换对话误报修复

起点 `a631bbf4`。写队列中的异步前驱原先需要再次被 Tokio poll 才能进入 writer；同步后继可能挡住
其所在调度线程，导致写线程空闲却不能排空，最终 RPC 接纳耗尽。现改为前序释放 ticket 时直接提交
就绪任务，保留原有 FIFO、容量、取消和 durable ACK 契约，见[存储执行架构](round3-storage-execution.md)。
Renderer 被动状态读取失败不再弹出主动模型切换失败提示，并合并在途读取、拒绝迟到的旧状态。

验证：旧实现两项真实磁盘无 repoll 回归失败，修复后新增 7/7、完整存储 1,234 项通过，1 项原有专项
release 测试按配置忽略。服务层检查点 16、模型切换 25、World State 5、生产启动 3 项通过；World State
的两条旧 fixture 使用仅限 debug 的初始化接口，因此该组在 debug 配置复测，未修改生产凭据边界。
前端 browser 24、unit 5 项和类型检查通过，前后端均已构建。独立临时库真实 bootstrap 连续运行
35 秒，1,338 次 RPC 全部返回成功并正常 shutdown，覆盖一个周期性 WAL 检查点间隔。

本次处理运行卡死，不重算历史性能指标；下述第六轮整体性能验收未通过结论保持。

## 第六轮：综合验证、修复与未通过门槛

状态：**正确性修复、独立复核和验收证据已交付；整体性能验收未通过，六轮优化不能标记完成。**

起点 `6e7bd1cc`；实现提交 `b1744ab9`，收集器、逐 Run 恢复核验与吞吐统计修补提交 `0d70c623`，诊断关闭入口修补提交 `d4390ac3`。
报告与证据另作本地文档提交，可用 `git log -1 -- docs/performance/backend-concurrency/PROGRESS.md` 查询。未推送、未发布。

- 修复陈旧模型历史快照的最终 CAS 降级、运行检查点的 generation/lease 围栏、通知提交与响应组装后的 lease 新鲜检查，以及持续刷新时旧页面覆盖与刷新饥饿问题。
- append 准备采用模型专用历史快照，复用原缓存预算；检查点重放共享未确认 payload。未扩大队列、线程或业务额度，未改变 durable ACK、权限、审批、工作流、归档/删除规则。
- 复用且核对前轮源码哈希，最终 **1,795 项不同 Rust 测试、1,849 次通过执行、0 最终失败**；129 项 unit、254 项 browser、两侧原生 Electron 夹具通过。收尾 28 项基础设施、类型/格式/lint、release Clippy 和文档检查通过。故障 helper 与重复执行不冒充新增场景。
- 36 场景两侧均为 1,608 RPC、1,572 成功、36 原容量拒绝、0 无回复；1,420 根和 158 子 Turn 的恢复分布相同。50 并发短/长历史发送 p95 **1002.674/2235.920 → 755.424/1106.118 ms**，50 并发短历史仍未达到 500 ms 门槛；两组 50 控制突发各有 18 次拒绝，不能写成全部自然排空。
- 相同 30 个终态的 Electron 对照：Renderer 全量回写 **293 → 0**，Host 请求字节 **10,369,347 → 48,046**；发送 p95 **21.4 → 19.6 ms**，但首次发送最大值 **43.4 → 199.9 ms**。不把低并发夹具当完整界面高并发 SLO。
- 修正后的 600 秒混合负载中，最终版本在 **239.861 秒**发生输出总字节超限；1,873 个成功 ACK 的身份与恢复一一对应，但未自然排空。4,346 个无 RPC 回执中，4,344 个为断连后夹具本地拒发、2 个为已启动 handler 未答。吞吐记为 `null`；旧失败结果和冻结环境修正前的运行也保留。
- 已证实的代价：同业务工作量矩阵 CPU **+38.7%**、采样 RSS **+9.7%**、线程峰值 **26 → 48**。完整历史响应与写 handler 的服务时间仍是性能阻塞，最大 SLO 内可持续负载尚未建立。

本轮涉及 Renderer 数据刷新与测试 Host/Renderer 链路，前五轮的 Host 窄查询和 Core Server 持久化所有权保持。视觉、文案、布局、图标、动画、入口和业务规则未改。
SQLite FULL 回滚、WAL 长快照下 Backup 恢复、3 个真实子进程强杀边界、10,000 facts/20 次服务重开均有证据；未认证 OS IOERR/fsync/断电和成套旧应用降级。代码回退不等于 schema 降级，不使用清库重建。

[第六轮完整结果、未通过项与复现](round6-results.md) · [验收口径](acceptance.md) ·
[最终证据入口](results/2026-10-12-round6/README.md)

剩余最小工作：关联并缩短发送 handler 内实际服务时间；为全文历史响应建立经过验证的有界背压路径；复测 50 短历史、持续混合负载和完整界面 SLO。必须保留最终 CAS、权限/lease、必要资源顺序和可靠落盘，不能以扩容或提前成功代替优化。
只使用隔离磁盘、合成历史、本机假模型与受控工具；未触碰真实用户库或计费服务，未中断用户软件。已有未跟踪缓存与截图未提交或删除。

## 第五轮：Core Server 持久化与 Host / Renderer 解耦

状态：**第五轮实现、正确性验证及实测交付完成，第六轮可以执行；已记录性能回退，整体优化尚未完成**。

起点 `df3e6f04ee1a8f14c8447b34841c76e50409ccbf`，实现提交 `e7237ce5216b27201022c7625cd236f2eb78f8c6`。
本页、所有权表、实测与原始证据另作本地文档提交；最终交付提交可用
`git log -1 -- docs/performance/backend-concurrency/PROGRESS.md` 查询。未推送、未发布。

已交付：

- Core Server 独立保存运行展示检查点，300 ms 合并窗口、每段 generation/sequence/fingerprint、未确认精确重放与终态围栏；沿用单 writer、双 reader 和既有有界执行器。
- 接纳事务保留显式 Skill 选择；普通和子 Agent 在无 Renderer 消费时保存正文、展示与游标。实际进程崩溃、取消、同步交互、保存失败和 ACK 丢失均有恢复验证。
- 等待批准/同步提问前完成必要持久屏障，封存旧段；终态、用量、模型上下文和授权决定仍由原事务处理，旧快照不能覆盖 Core Server checkpoint owner。
- Renderer 精确绑定 Core Server 所有权后取消运行消息全量写回与无关前置 flush；本地输入、草稿、metadata/UI 保存保持，失败队列保留 payload，显式 flush/退出传播错误。
- 协作审批按真实事件失效，组织树、审批和观察器读取 single-flight/尾随合并；Host 文件证明使用同快照窄 DTO，完整历史读取仍有界。

验证与实测：

- 43 组 Rust release 回归：**1,732 项不同测试、1,739 次执行通过，0 失败，1 项原有 ignored**；确定性恢复屏障另 20 次重复通过。Renderer/刷新 224 项浏览器测试，主要单测分组 43/31/54 项及 1 项真实 Electron 夹具通过；类型、lint、格式、文档及 22 项基础设施检查通过。
- **36/36** 同机同输入场景通过；1,608 RPC = 1,572 成功 + 36 原容量拒绝，0 无响应。1,420 根 / 158 子 Turn 的恢复终态逐场景与第四轮一致，重开后 0 InProgress。
- 同一浏览器流式夹具的全量运行消息保存 **4 → 0**；21 次刷新失效请求 **21 → 2**，并发峰值 **21 → 1**。100 个 durable 事件中 10 个审批事件，仅刷新审批 10 次。
- Electron 文件证明读取：同 256 条历史、各 20 请求，响应总字节 **34,694,335 → 5,500**；Host parse 合计 **13.985 → 0.0331 ms**，Host CPU **53.619 → 1.355 ms**，proof 与删除语义一致。
- **代价与残余：** Core Server 50 并发短/长发送 p95 **610.047/968.119 → 704.033/1067.709 ms**；审批 **132.562 → 131.399 ms**，停止 **70.179 → 70.456 ms**。稳态 p95 **283.371 → 394.160 ms**、最大值 **1277.824 → 497.099 ms**。
- 新增无前端持久化后，全矩阵 CPU **+21.8%**、RSS **+3.2%**；已埋点 DB 等待 **93.442 → 111.009 s**，writer 估算 payload **895.7 MB → 1394.9 MB**。这不是物理磁盘写入，仅运行 Core Server 的基线也未包含旧 Renderer 全量写回负载，不能宣称全栈净写入或整体延迟改善。

确实改到 Electron Host、前端数据调用/保存/订阅/恢复逻辑；产品视觉、文案、布局、图标、动画、入口与业务规则保持。
Provider preflight 和 Core Server 最终配置/权限/版本校验保留，未增运行额度、线程或传输队列。

[所有权与恢复契约](round5-persistence-ownership.md) · [完整实测与限制](round5-results.md) ·
[原始证据](results/2026-10-11-round5/summary.json)

第六轮可从本轮提交继续长稳态、磁盘故障、租约及端到端 SLO 门禁；不代表六轮整体完成。
仍有 Done 权威全历史回读、少量同步保存屏障、单 writer 争用和完整安全 JSON 序列化成本。
未测得可归因的物理磁盘累计写入字节；writer reservation 的估算字节与 Host 传输字节分别报告，不能混用。

本轮仅使用隔离磁盘夹具、合成历史、内存凭据、loopback 假模型和受控工具；没有触碰真实用户库或计费服务，
没有中断用户软件。复用同一隔离 target，已有未跟踪缓存和截图不提交、不删除。

## 第四轮：资源排序与准备期并发

状态：**第四轮实现及本轮验证完成，第五轮可以开始；后端整体优化尚未完成。**

工作起点 `06fbdb60`（第三轮生产源码 `354762d6`），实现提交
`e787b88d045549ea5d204f5c01f77f391b7cfc49`（`refactor(core): scope turn preparation and resource scheduling`）。
结果与原始证据另作本地文档提交；用 `git log -1 -- docs/performance/backend-concurrency/PROGRESS.md` 查询最终交付提交。
未推送、未发布；未改 Host、Renderer、前端保存责任、业务额度、权限和审批规则。

已交付：

- Core Server 解析完整资源归属，按 Conversation/Run/Action/Tree/Project 排序；冻结与当前 Project 同时声明，归属变更后重新解析，未知资源保守全局排序。
- Preparing boot/generation、取消与删除围栏，writer 内最终身份/权限/revision 检查；运行期恢复只处理精确已终态 Run，不清理其他尚未提交 Trace 的 claim。
- Run 发布/审批/组织持久作用按 Run 排序，账号队列入口冻结身份，MCP 批量失效完整键预约；小 registry 不跨慢 SQL 等待。
- 删除登记范围内真实活动并排空，Project 最终事务重查完整成员集合；补齐 terminal 与删除的反向锁序、关机后继 generation 及未知新会话准备取消的竞态。
- 每 lane 的 root 公平调度、子 Agent 同 root 单个 claimed 准备；普通 Write handler 1 → 2，共享身份解析 worker 2，原 RPC 数量/字节预算与逻辑额度 50 保持。

验证与收益：

- 最终 **1,622 项不同 Rust release 测试、1,628 次通过、0 失败**（37 组），1 项原有 ignored；Rust 格式、release Clippy（既有四类命令行排除）、文档、测试布局及 22 项测试基础设施检查通过。
- **36/36** 同机同负载场景通过，运行期间源码一致；1,608 RPC = 1,572 成功 + 36 原有容量拒绝，0 无响应。
  1,420 根 Turn / 158 子 Turn 的持久终态与第三轮逐场景相同，重开后 0 InProgress。
- 50 并发短/长历史发送 p95 **856.414 / 1388.878 → 610.047 / 968.119 ms**；50 并发审批 p95 **178.825 → 132.562 ms**。
- 50 长历史准备后的独立审批 **1434.417 → 8.261 ms**（各一次观察）；1/10/30 秒真实慢工具期间，其他 4 根的审批及终态均在慢工具退出前完成。
- 固定输入稳态 620/620 成功，响应 p95 **397.067 → 283.371 ms**，完成速率约 10.27 Turn/s；不是最大吞吐测试。
- **代价与残余：** 累计已埋点 DB 等待 **34.532 → 93.442 s**；50 并发 Stop p95 **50.135 → 70.179 ms**，稳态响应最大值 **662.566 → 1277.824 ms**、timer 最大迟滞 **50.599 → 238.897 ms**。
  CPU **+1.5%**、采样 RSS **+4.6%**，采样线程峰值 **63 → 42**。新增物理并行度的成本已纳入；不能称所有延迟或数据库性能改善，也未达到全部 SLO。

[资源调度契约](round4-resource-scheduling.md) · [完整实测与限制](round4-results.md) ·
[原始证据](results/2026-10-11-round4/summary.json)

第五轮先明确 durable journal/cursor 与 Core Server/前端持久化和发布责任，再逐入口收敛同步桥与重复保存。
单 writer 争用、账号撤销的整批 Control 作业、未审计全局操作、默认 blocking pool 总预算和本轮尾延迟回退仍需处理。
第六轮保留长稳态、崩溃/磁盘满、租约及端到端 SLO 门禁；有限双 root 与慢工具夹具不证明持续负载永不饥饿。
矩阵没有独立逻辑占用峰值字段，不能用 provider 并发或累计完成数代替额度验证。

本轮只使用合成磁盘库、内存凭据、loopback 假模型及受控工具，未触碰真实用户库、计费模型或运行中软件。
复用约 2.9 GiB 的隔离 target，夹具自动清理，保留约 2.5 MiB 证据；原有未跟踪缓存和截图未提交或删除。

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
