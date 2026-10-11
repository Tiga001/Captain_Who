---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-10-11
---

# 第二轮：阻塞执行、租约与关机边界

本轮基于 `6c421d07`，源码交付至 `d7b3fbaf`，沿用第一轮指标、磁盘夹具及业务并发契约。性能及验证结果见 [第二轮结果](round2-results.md)。
本页说明临时阻塞桥接器，不代表第三轮单写通道、读取池或第四轮资源级并发已经完成。

## 执行能力与业务额度

模型及异步工具仍由 Tokio 独立运行。新增固定 OS worker 只承接有限的同步阶段，不把整个模型 future
移入工作线程，不扩大现有 50 个逻辑 Turn 上限。等待批准、等待交互的逻辑 permit 释放规则保持原有实现。

| 通道         | 物理线程 | 接纳数上界（含 reservation / 排队 / 执行） |                               估算字节预算 | 用途                                                       |
| ------------ | -------: | -----------------------------------------: | -----------------------------------------: | ---------------------------------------------------------- |
| preparation  |        2 |                                          8 |                                    128 MiB | 子 Agent 启动、运行时 host 准备、交互恢复、组织投递扫描    |
| maintenance  |        1 |                                         64 |                                     16 MiB | claim、恢复扫描、持久观察、结算；不放等待 admission 的取消 |
| lease        |        1 |                                         64 |                                      4 MiB | 仅续租，不排在 claim 投影和恢复扫描之后                    |
| control      |        2 |                                         32 |                                     16 MiB | dispatcher 取消、关机围栏                                  |
| completion   |        2 |             `2 × 原逻辑上限 + 8`，默认 108 | `max(8, 原逻辑上限) × 8 MiB`，默认 400 MiB | 持久终态、重试及原有清理                                   |
| recovery     |        1 |             `2 × 原逻辑上限 + 8`，默认 108 | `max(8, 原逻辑上限) × 8 MiB`，默认 400 MiB | 已落盘终态之后、需要全局 admission 的回执恢复尾段          |
| RPC Control  |        2 |                            原 transport 32 |                    沿用 transport 请求边界 | 停止 handler 和归属解析                                    |
| RPC Approval |        2 |                            原 transport 32 |                    沿用 transport 请求边界 | 审批 handler 和归属解析                                    |

预算是接纳权重，不是预分配，也不是实际 RSS 硬限制。准备前尚未加载的历史采用固定保守估算；已拥有的
运行输入按字符串、历史及附件信息估算。大历史、输出增长、Arc 共享、SQLite cache 和已有模型 future
不受此数字精确约束。RPC 没有复制第二份完整请求队列；仍使用原 transport admission 与排序依赖。

线程依据：第一轮测试机为 M4 / 10 逻辑 CPU / 16 GiB，50 并发子树发生数百毫秒 Tokio 迟滞，且现阶段
准备仍受全局 admission 串行保护，增加几十个准备线程无法增加安全并行度。准备选 2，维护和续租各选 1，
控制和完成各 2；沿用 RPC Control / Approval 原执行数 2。终态 recovery 另用 1 个固定线程，依据是首次矩阵及定向复测发现每 Turn 10ms 重试造成数千至上万次 completion 排队；集中阻塞等待原 admission 避免轮询放大，也不占用控制、续租、准备和落盘线程。容量不会授权新 Turn。
最终实测线程、RSS、延迟和吞吐必须以第二轮结果页为准，不能从选值推断已经达到产品 SLO。

## 接纳与所有权

1. `try_reserve(bytes)` 同时检查数量与字节，过载明确返回 `Overloaded`，单作业超过预算返回 `TooLarge`，
   关闭返回 `Closed`。没有静默丢弃或默认退回无限 `spawn_blocking`。
2. `reserve().await` 只供已受逻辑 Turn / transport / 单一 scheduler owner 限制的调用方等待预算。
   等待期间不建立另一份闭包队列，不得把它用于无界入口。其已有输入仍由外层 owner 持有。
3. 获得 reservation 与 durable admission 是两件事；入池不代表持久接受、开始模型或业务成功。
   原 RPC 仍等待原 handler 真正返回，不提前 ACK。
4. worker 启动前显式 `cancel_before_start` 可阻止闭包运行；与 started CAS 竞争失败则不能宣称已取消写入。
   丢弃 `BlockingTask` 只丢弃等待端，任务、预算和同步变更继续由 worker 持有到实际结束。
5. panic 转为明确未知结果错误，释放预算且 worker 继续工作；调用方依赖现有 Trace、claim、receipt 恢复，
   不把 panic 推断成事务必然回滚或副作用必然没发生。
6. worker 进入提交方 Tokio Handle，仅为了原有 `tokio::spawn` 和有限结算重试 timer；模型/网络等待不占物理槽。
7. 每段执行前一次性取得终态与后续投递的两个 completion reservation，数量和字节在同一锁下检查与预留，
   不能先占一半再等另一半。后续投递即使已释放逻辑 Turn permit，仍持有第二份预算直到同步扫描与 Done 完成；
   它不占 completion 物理线程，关闭时也受 late-submit fencing 约束。

诊断继续使用第一轮 `performance` registry：`blocking.queue_wait`、`blocking.execution`、
`blocking.estimated`（数/估算字节/最老等待）及 `turn.child_preparation`。估算 queue 标签不得与 wire byte
或实际堆内存测量混淆。固定 registry 若有 omitted 必须在性能结果中报告。

## claim 与租约

子 Agent 调度先预留 preparation 容量与原逻辑额度，claim 成功后才将逻辑预留转为 active；准备容量耗尽时保留
SQLite 中的 queued Wake，不提前占 claim 或额外创建一份无界内存队列。

claim 后独立续租覆盖排队、准备、提交与运行观察。lease 与 preparation、maintenance、control 分离，所以慢文件
准备或 admission 锁等待不会耗尽续租的物理工作线程；仍共享单 SQLite connection，连接占用过长仍可能失租。
失租必须 fail closed，不延长 TTL 掩盖问题，不使用缓存向失效 token 授权。

trusted Wake admission 在等待连接前采样 wall/monotonic 时间，在获得事务、绑定 CAS 和 commit 前重查有效期
及既有精确身份。不能把较早 `trace_updated_at` 当作提交时刻。换 token、tree stop、缺失或过期 claim 仍拒绝。
Trace 时间保持原值；不改变权限继承、树停止 generation 或事务原子范围。

生产续租、等待审批状态推进及结果 Outbox 结算也使用连接/事务等待后的新鲜时间，并在提交前再次检查原租约。
显式 `_at` / repository timestamp API 继续保留可控测试时钟；已提交的终态幂等回执仍可在租约过期后读取。
初始及审批续跑冻结原 claim capability，运行时 host 准备结束、真正调用模型前再次核验 exact token、run、assistant、
源消息、active Agent、in-progress Trace 和 tree-stop。不会在末尾读取新 token 来替旧准备重新授权。

最终准备校验拒绝后，旧段不写失败终态，也不发布 Done。进程内按 wake / claim / run / assistant 精确记录拒绝，
停止原 token 的续租与重复接管，交给 SQLite 既有过期恢复。该表受原逻辑额度约束，终态、删除、换代后清理；
普通登记仅操作内存，避免每次准备扫描全部在跑的 Agent。失租只取消原 cancellation token，不能按复用的 run ID
取消接手者。进程重启不依赖此内存表授权，仍从持久事实恢复。

模型运行或 completion 排队期间都可能失租。可信子 Turn 的消息、Trace、模型上下文、用量与通知仍在原事务中
提交，并带冻结的 claim 校验及提交前期限重查；旧 token 不得把接手者写成 Completed、Failed 或 Cancelled。
合法树停止的当前 token 仍可写取消终态，控制与恢复入口保留原有权威路径。
已提交终态的重放仅允许所有提交字段一致、且不产生用量/通知/回执写入的 no-op；消息和 Trace 一致但用量不同仍拒绝。
可信子 Turn 的终态用量先在局部副本累计，fence 前不修改共享 accumulator，也不在拒绝或重试时恢复旧 owner 快照；
持久化成功后按原语义清理。该事务保护覆盖 Completed / Failed / Cancelled；待批准和等待交互仍使用既有
suspend / pending 身份 CAS，不代表同步 emitter 已全部改造。

组织投递扫描在进入同步扫描、可能 claim 之前取得 preparation 容量。原 workflow version、输入绑定与
admission 围栏保持不变，不把普通 durable workflow claim 错当作子 Agent TTL lease。

## 锁与有限线程

- 现有主要锁顺序保持 `conversation_admission → execution_access → SQLite`；撤销路径仍以原访问锁与数据库
  事实线性化。没有最终重查与 Preparing 协议之前，不提前放开权限或 admission 锁。
- async 调用者提交闭包时不持有闭包会获取的业务锁；锁在 worker 内获取并释放。
- preparation 与 completion 不共用物理池。删除等待结算时，不能让等待 admission 的准备占完结算线程。
- 原终态 recovery 必须保留 admission 围栏，避免误回收已 claim、未写 Trace 的准备。等待该锁不能长期占满
  completion 槽位，否则会新增删除 drain 与结算之间的循环等待。原子终态提交后先注销精确旧 owner、逻辑 permit
  和 cancellation，再将需要全局恢复的尾段交给一个独立 recovery worker 等待该锁。全程保留原 Turn 的第二份
  completion 预算，不产生每 Turn 轮询任务；Done 仍等待回执恢复及投递。待批准、待交互及失败持久化等不需要
  admission 的清理留在 completion 内完成，避免删除持锁等待它们时反向依赖 recovery。
- maintenance 不承接 `interrupt` 或其他可能等待全局 admission 的长作业；其中 claim 的 mailbox 投影和
  recovery 扫描仍可能较长，因此续租使用额外固定 lease worker，不与其共用执行队列。
- 同步 Runtime emitter / Trace observer 仍有 SQLite 回调，普通 RPC Write 也保留全局顺序。这些是第三、四轮
  明确迁移项，不能说本轮消除了所有同步阻塞或独立对话间所有排队。

回答提交仍同步完成原有投递路由，不能把 RPC 返回后的已接受回答延后到下一次模型采样。运行终态的后续投递
由原 Turn 等待 preparation 作业完成，不能作为脱离生命周期的后台扫描跨越重启。Done 在持久终态、owner 清理、
回答回执和该次投递处理之后发布。等待 admission 的投递不得占用 completion 线程。

## shutdown

先停止外部接纳及 scheduler 新 claim，保留续租/取消/结算能力。已开始的同步准备不可通过 abort observer
来假装停止；必须接管其真实结果，必要时取消晚到 run，保留确切 token 和恢复事实。异常 manager 退出也必须
遵守此规则，不能只覆盖正常 shutdown。

`BlockingExecutor::shutdown()` 等待已发 reservation 及已提交作业 drain。进程最终边界的 `shutdown_submitted()` 把
关闭与 late-submit fencing 放在同一个 state 锁下：已入队/正在执行的同步工作排空，尚未提交的 reservation
以后提交得到 `Closed`。因此超过既有 grace 的模型 future 不会无限拖住同步执行器关机，也不能再通过这些已关闭的执行器
提交同步变更。仍未完成的逻辑运行依据已有持久状态恢复，超时不转换为成功。

关机在 admission 锁内关闭自动回答启动，同步恢复在 cancellation 注册锁内重查同一关机标志；已经注册的恢复
会进入关机取消快照，尚未注册的回答保留 pending 供重启。终态投递尾段不得在快照之后另起未受管理的模型任务。

最终执行器关闭顺序为 preparation → control → completion → recovery → maintenance → lease。completion 先排空；
尚未提交的 recovery 或投递尾段遇到关闭会明确报错并保留已提交终态、claim 和回执作为恢复依据，不伪造 Done。

不能以 cancellations map 已空替代实际 mutation drain：终态 worker 可能刚注销 token，仍在完成通知尾段。
实际排空必须包含 submitted count 和 worker 退出。关机不承诺能强制中止任意不可中断的系统调用。

## 第三、四轮清理任务

1. 将桥接器内 SQLite 调用迁入专属短事务 writer / 小读池；先补 after-commit 通知及一致快照。不能永久再叠一层
   storage 队列而继续占满这里的线程等待，迁移时逐入口删除临时桥接。
2. 拆出同步 emitter / Trace observer 的持久 API，调用者必须等真正提交后发布审批、终态及恢复事实。
3. 将 claim、prepare、最终 CAS、recovery 统一到资源范围 owner 协议后才移除全局 admission / RPC fence。
4. 以同一基准重测 tail latency、吞吐、公平性与内存；短矩阵和有限竞态不替代长稳态、磁盘满或断电验证。
