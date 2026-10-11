---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-10-11
---

# 第四轮实测结果与验收

第四轮资源排序与准备期并发已实现，完成本轮正确性回归及完整性能矩阵。第五轮可以开始；后端整体优化尚未完成。
独立会话发送和慢准备后的审批响应改善，同时数据库排队、部分停止/稳态尾延迟及 CPU/RSS 有回退，详见下文。
正式实现边界见 [资源调度契约](round4-resource-scheduling.md)。本轮仅本地提交，未推送、未发布。

## 交付范围与源码

本轮工作起点为 `06fbdb60`（第三轮生产实现 `354762d6fae3f8959027b10069a972c9b44b427f`），实现提交为
`e787b88d045549ea5d204f5c01f77f391b7cfc49`（`refactor(core): scope turn preparation and resource scheduling`），
包含 74 个 Rust 文件及 3 个文档文件。证据和结果文档另作本地提交，可用
`git log -1 -- docs/performance/backend-concurrency/PROGRESS.md` 查询最终交付提交。

交付第四轮的 Core Server 资源归属、按资源排序、Preparing generation 与最终接纳检查、精确 Run 恢复、Run 发布和审批范围、账号入队身份冻结、
Project 已排空成员集合的最终事务 CAS，以及 root 准备公平调度。没有修改 Electron Host、Renderer、前端保存责任、页面交互、业务额度、
权限/审批语义或业务 schema；第五轮的持久化责任收敛及第六轮全面验收尚未完成。

普通 RPC Write handler 槽由 1 增至 2，增加共享的 2 个身份解析 worker。Read/Control/Approval 槽分别保持 4/2/2，RPC 接纳数量和字节预算不变，
全局逻辑运行额度仍为 50。物理执行槽与逻辑 Turn permit 是不同预算，新增容量必须纳入性能和资源比较，不能将收益全部归因于锁范围变化。

验证仅使用临时磁盘库、合成历史、内存凭据、loopback 假模型和临时目录内的受控工具。没有读取或修改真实用户库，没有调用计费模型、
发送真实对外消息，或操作正在运行的软件。复用约 2.9 GiB 的 `/tmp/captain-backend-baseline-20261011-target` 隔离构建缓存，临时磁盘夹具自动清理；
保留约 2.5 MiB 原始证据，原有未跟踪缓存和截图未提交或删除。

## 最终正确性结果

最终接受的是 `regressions/accepted-regression-summary.json` 中完整的 37 组：**1,622 项不同 Rust release 测试，1,628 次通过，0 失败**，
另有 **1 项原有 ignored**。其中 Rust Core storage 1,176 次、Rust Core performance 7 次，Core Server 35 组共 445 次。
六个测试分别被两个筛选条件覆盖，因此执行次数比独立测试数多 6；没有把重复执行算成新增测试。

唯一 ignored 为既有已登记的
`storage::service::child_agents::tests::model_recovery::release_profile_tree_mailbox_event_contention_and_restart_recovery`。
本轮没有新增 ignored，也没有通过跳过失败测试取得最终结果。

摘要由脚本核对 0–36 组齐全、组编号不重复、日志测试数与声明一致、各组退出码为 0，并核对接受日志记录的二进制 SHA-256。
最终回归二进制为：

| 二进制                            | SHA-256                                                            |
| --------------------------------- | ------------------------------------------------------------------ |
| `core_server-f7a97b10b9960ccf`    | `bd0bef84129ac278252b442a312404e7743f178f526de15d9a6df1ebf194f48e` |
| `mycopilot_core-7748a31bd053ad1f` | `9b2666924fad9695ceed221bd7c49c2acfc1430bb777f74cb410283849af5bbc` |

Rust Core 两组日志耗时合计 331.487 秒，Server 各组耗时合计 61.926 秒。这是回归执行耗时，不能当作业务吞吐或矩阵运行时间。

关键覆盖：

| 边界                 | 最终验证内容                                                                                                                                                                                                                                 |
| -------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Preparing 身份和取消 | 同 Conversation 一位 owner、不同 Conversation 并行、Run 不得别名、旧 boot/generation 不退休后继、取消保留真实所有权、阶段/revision 固定、删除围栏、无丢失唤醒的关机排空                                                                      |
| 真实 Turn 接纳       | 两次同会话发送恰有一位赢家；取消/删除不留下初始消息和 Trace；历史或模型 revision 变化拒绝陈旧输入；同会话模型切换排序、无关会话继续；权限撤销和退出重登录不能恢复旧准备                                                                      |
| 完成与 claim 恢复    | A 的真实完成尾段不扫描或终结 B 已 claim 但尚未提交初始 Trace 的 Preparing；B 完成及重开后 receipt 保持正确；启动全局孤儿恢复与运行期精确 Run 恢复分开                                                                                        |
| 资源排序和公平       | Core Server owner 校验、防伪 DTO、Frozen/Current Project 同时排序、移动后 scope 刷新、未知全局屏障、精确读写双向顺序、概览读取、独立写并行、每 lane 的 root 轮转、数量/字节及取消归还；同 root 的持久 `claimed` 限制与不同 root/人类工作推进 |
| 账号发布             | 队列入口冻结 token 或拒绝结果；线程局部作用域在返回/异常后退出，不泄漏到 service clone；撤销越过正在运行和排队的准备而不重新授权它们；账号 registry 不随 writer 等待长期持有                                                                 |
| 发布、审批和终态     | 慢 Run 的发布不阻塞另一 Run 的取消；同 Run 的问题/审批发布与 Stop 有序；旧 segment 不能覆盖后继；关机后新注册 generation 立即取消；MCP 批量失效完整键集合、身份精确投影；组织持久发布与本 Run Stop 有序                                      |
| 删除和真实作用       | 删除等自己范围的准备/审批/文件/终态活动，不扩大为同项目普通会话的全局等待；移动成员后等待冻结旧 Project 的作用；未结算 effect 或排空超时保留数据；真实终态保存前不持 registry 等 writer，无关写继续，活动未退休时拒绝删除，释放后可重试      |
| Project 最终集合 CAS | 普通与 Agent 树删除覆盖新增、移入、移出、移除、漏成员；错误快照被拒绝，独立只读连接核对全部应用表无变化；正确完整快照允许重复/乱序，删除并重开核对无关数据仍在                                                                               |
| 历史协议             | 全量 Rust Core storage 回归继续覆盖 WAL、读快照、提交后通知、完整领域事务、租约、Trace 和恢复；Server 继续覆盖同步/异步人工输入、待审批、取消、自动化、子树、Provider 切换、上下文视图、调度、阻塞执行器和真实 request loop                  |

Preparing service 集成 12 项、KeyedAdmission 8 项、Preparing 单测 7 项、账号状态 10 项、RPC scope 8 项、bounded dispatcher 37 项均包含在上述最终总数中，
不是额外相加的测试。Project 集合 5 项测试分别运行普通/Agent 树两种分支；这 10 个情景不是 10 项独立 Rust 测试。

## 早期失败与修复

`early-regressions`、`pre-final-smoke`、`pre-final-regressions` 和 `sixth-regressions` 仅用于诊断。它们包含不同源码/二进制状态，不能并入最终通过数或性能样本，
即使其中某次全部通过也不能替代最终 `regressions` 目录。故障注入测试预期出现的 worker panic 日志不等同测试失败，以完整测试结果为准。

1. 初次合并构建暴露测试 import、新函数参数/资源字段未补齐、Receiver 不满足共享 hook 的 `Sync` 约束，以及 `JsonRpcRequest` 不支持整体序列化。
   接线修复后，identity resolver 只复制审计过的小型查找提示；共享测试 Receiver 由 mutex 保护。这些编译失败没有进入最终测试计数。
2. 初次 Preparing 撤权/重登录回归虽然阻止接纳，但 rollback helper 把原 `AgentServiceError` 转成字符串，丢失结构化账号错误码。
   修复为保留原错误及 data，回归直接检查 `ACCOUNT_LOGIN_REQUIRED` / `ACCOUNT_LICENSE_UNAVAILABLE`，没有依靠更宽松文案断言放过错误。
3. 真实 workflow claim 夹具没有使用受信任输入对应的规范 `workflow-message-{input.id}` 消息身份，触发原有 admission 校验。
   夹具改为真实匹配身份，保留完整受信任校验、claim 及完成恢复断言。组织发布隔离夹具另缺活跃 assistant Trace；改为先完成 service 启动恢复，
   再登记真实 assistant/Trace 和绑定，避免把准备中的夹具当作启动遗留状态恢复。
4. 上下文一致性、MCP 审批和 request loop 的部分旧测试在 release 下使用隐式凭据后端，触发生产防护。
   统一为测试显式注入内存凭据；没有放宽生产凭据要求、HTTP 连接策略或隔离数据库边界。
5. 延迟审批投影夹具直接尝试不匹配当前状态的持久状态迁移，数据库 CAS 正确拒绝。
   修正为真实 Pending → Executing → Cancelled 顺序后，仍断言迟到的 Pending → Executing 投影被拒绝且持久/内存均保持 Cancelled。
6. Clippy 发现只在测试使用的 Preparing helper 被生产编译为 dead code，以及布尔值与 false 比较。
   helper 限于 `cfg(test)`，断言使用直接取反；没有增设 lint 全局豁免。

实现审查另补齐了下列并发缺口，并将回归纳入最终二进制：

- 关机先快照 token 再等待 Run 发布 gate 时，后继 generation 可能替换旧 token。现于取得 gate 后取消当前 token，晚注册在同一 gate 内检查停止围栏并立即取消。
- 长准备尚未公布完整 scope 时，排队准备可形成撤销的间接前序依赖。transport 单独公布稳定的准备类型，让账号撤销通过准备依赖，随后按入队身份拒绝旧请求。
- 初始 Conversation metadata 缺失时，只对 Core Server 已登记的精确 Preparing owner 开放已开始准备的 Stop 通路；未知/未登记 owner 继续保守全局排序。
- 删除 Control writer 取 deletion registry 与终态持 registry 等 writer 形成相反锁序。root terminal、续跑及对应发布改为短登记 lifecycle activity，保留真实 drain guard，释放 registry 后等待 SQL；失败终态事件也由活动范围覆盖。
- Office 概览查询先前以未知全局读屏障阻挡控制；真实 request loop 的慢读取回归发现后，仅将经审计的 `office.getStatus` 定为 Overview，允许后续无依赖控制前进；未知/精确读写及前序全局写的围栏保持。
- Project 排空期间成员可移动，单靠初始列表可能删除未排空成员。现捕获完整集合、同时围栏 Project/Conversation，并在最终事务内再次比较，变化要求重试。
- 取消/pending registry 的长 SQL 和组织发布改为按 Run 的 gate；MCP 批量失效一次预约完整 Run 集合，等待后发现新增 owner 则释放后重试，避免嵌套升级。

## 静态检查和测试基础设施

- 74 个变更 Rust 文件格式检查通过（`format-final.log`）。
- release Clippy 全 targets 通过，沿用已有 `collapsible_match`、`useless_conversion`、`unnecessary_lazy_evaluations` 和旧 trace API 的 `deprecated` 四类命令行排除。
  不称为无条件全仓零告警；没有改项目 lint 配置。最终构建仍记录 Rust Core 测试使用旧 trace API 的警告。
- 文档检查通过，storage schema v71；测试布局和 ignored 登记检查通过（502 个测试文件归属、15 项全仓 ignored 登记）。最终日志见归档。
- 测试基础设施 22/22 通过，0 skipped。比较脚本 self-test 验证已有 36 个 case 可核对；终态分布变化作为观察报告，扩大额度或缺失已接纳回执必须报错。
- 最终文档格式、链接/索引检查及 `git diff --check` 通过；不以格式检查替代行为回归。

全仓 ignored 登记的 15 项与本次选择执行中遇到的 1 项含义不同，不能混为本轮跳过数。Host/Renderer 未改，本轮没有用新的 UI 检查声称端到端验收。

## 同机、同处理量的完整矩阵

第四轮 `e787b88d` 的有效运行是 `round4-full-20261011-r1`，比较第三轮 `354762d6` 的 `round3-full-20261011-r1`。
同一台 Apple M4、10 逻辑 CPU、16 GiB，release、相同 36 个负载、三次普通波次和 2 秒间隔。逻辑运行额度仍为 50，RPC 接纳数量和字节预算未扩大；
普通 Write handler 槽从 1 增至 2，新增 2 个身份解析 worker，属于本轮必须一并计价的物理容量变化。

本次 runner 自行构建测试二进制，构建与完整运行退出码均为 0，运行期间源码指纹未变化。矩阵用时 **206.016 → 200.244 秒**，约减少 2.8%；
这包括既定波次、60 秒固定输入和慢工具等待，不能按总用时推断最大吞吐。比较报告无内部完整性错误、无负载不一致、无聚合成功/拒绝/终态分布变化。

两轮实际处理量完全相同：

- **1,608 RPC = 1,572 成功 + 36 个既有容量拒绝，0 无响应。** 50 并发审批、停止各提供 50 请求、接纳 32、拒绝 18；没有把拒绝从分母中隐藏。
- **1,420 根 Turn = 1,316 Completed + 104 Cancelled；158 子 Turn 全部 Completed。** 成功接纳的根 Run 身份与重开持久终态逐项核对。
- 50 并发短/长历史分别 150 次发送全部成功；固定输入稳态 620/620 成功。
- 重开后均为 **0 InProgress**。关闭前 18 InProgress 同样来自预期审批/停止过载情景，随后由既有关闭/恢复流程结算。
- 非预期 RPC 错误、关机超时、完整性错误和诊断 omitted 均为 0。两个 50 单元审批/停止场景的清理前队列快照仍未 drain，不能把持久终态等同所有运行尾段已退出。

响应指标以 **Core Server 入口计划发送时刻到成功 RPC 回执** 为边界，不含 Electron 点击、跨进程 UI 往返或 React paint。
分位数使用原始成功样本的 nearest-rank；拒绝与无响应计数另列，n=1 只记单次观察。
50 并发审批虽然均成功 32 次，但成功 owner 集合并非完全相同：第三轮成功集合含 owner 30，第四轮改为 owner 32。
因此这是同等输入和处理量下成功响应分布的比较，不能称逐 owner 配对改善。

| 指标                             |        第三轮 |        第四轮 | 样本与变化                                   |
| -------------------------------- | ------------: | ------------: | -------------------------------------------- |
| 50 并发短历史响应 p95            |    856.414 ms |    610.047 ms | 各 150 成功，降低 28.8%                      |
| 50 并发长历史响应 p95            |   1388.878 ms |    968.119 ms | 各 150 成功，降低 30.3%                      |
| 50 并发审批响应 p95              |    178.825 ms |    132.562 ms | 各 32 成功，降低 25.9%；另各 18 容量拒绝     |
| 50 并发停止响应 p95              |     50.135 ms |     70.179 ms | 各 32 成功，**回退 40.0%**；另各 18 容量拒绝 |
| 50 长历史准备后的独立审批        |   1434.417 ms |      8.261 ms | 各 1 次观察，不称 p95                        |
| 上一行 transport dependency 等待 |   1431.343 ms |      0.013 ms | 各 1 次观察                                  |
| 固定输入稳态响应 p95             |    397.067 ms |    283.371 ms | 各 620 成功，降低 28.6%                      |
| 固定输入稳态响应最大值           |    662.566 ms |   1277.824 ms | **回退 92.9%**                               |
| 固定输入稳态完成速率             | 10.248 Turn/s | 10.266 Turn/s | 固定输入完成率，不能称最大吞吐提升           |
| 全矩阵 CPU user + system         |      74.924 s |      76.019 s | **增加 1.5%**                                |
| 采样 RSS 峰值                    |   307.766 MiB |   321.906 MiB | **增加 4.6%**                                |
| 采样线程峰值                     |            63 |            42 | 低频采样，不能称精确峰值或总线程预算         |

改善集中在普通发送的排队和跨资源审批等待，未表现为全面尾延迟改善。稳态 timer p95 基本相同，**3.797 → 3.773 ms**，
但最大值 **50.599 → 238.897 ms**，增加约 3.72 倍；稳态 RPC 最大值也接近翻倍。50 并发审批的 p50 从 **27.472 → 45.977 ms**，
尽管 p95 降低，不能称其整个分布都更快。20 并发审批 p95 **30.763 → 54.932 ms**，回退 **78.6%**；
固定输入稳态 CPU 增加 **8.1%**，高于全矩阵的 1.5%。

timer 的其他变化也混合：50 长历史最大值 **35.544 → 3.582 ms**、50 并发审批 p95/最大值
**13.673/59.076 → 3.641/40.812 ms**；50 短历史最大值 **2.106 → 3.118 ms**，50 并发 Stop 最大值 **2.547 → 3.552 ms**。
这些是本次样本事实，不作宿主调度、存储或新增 worker 的单一归因；没有对这些因素分别做消融实验。

### 排队位置变化与数据库代价

50 并发短历史从实际 offer 到 handler 开始的排队 p95 **826.068 → 572.284 ms**，长历史 **1356.947 → 912.209 ms**。
全部 1,420 个普通写 RPC 的 `rpc.queue/write` 累计 **378.244 → 264.384 秒**。独立审批在慢准备之后的单次 dependency 等待降至 13 微秒，
与资源排序解除跨会话前序依赖的实现一致；同次审批的 owner resolution 用时为 5.429 ms，回执为 8.261 ms，仍有解析与持久工作。

不能只比较 `rpc.lane_wait` 就宣称排队消失。第四轮先完成 owner/依赖解析，再等待公平 handler 槽，原先的 lane 排队大量转移到
`rpc.execution_permit_wait`：全部普通写 RPC 的该段累计为 **260.043 秒**，新增 owner resolution 累计 **4.275 秒**。
这些是 `rpc.queue` 内部阶段，不应再与总 queue 相加。50 长历史 handler execution 的 p95 **桶上界**反而从 **49.151 → 57.343 ms**；
短历史保持 **36.863 ms**。阶段直方图的桶上界不是精确成功响应分位数。

下列数据库统计沿用第三轮口径，均为所有场景内已埋点调用的累计值，不是矩阵墙钟时间：

| 数据库指标                |   第三轮 |   第四轮 | 解释                                         |
| ------------------------- | -------: | -------: | -------------------------------------------- |
| 累计 DB 等待              | 34.532 s | 93.442 s | **增加 170.6%**                              |
| 其中 writer queue         | 33.096 s | 83.345 s | **增加 151.8%**                              |
| 其中 reader wait          |  1.180 s |  9.833 s | **增加 733.0%**；读租借 151,785 → 155,934 次 |
| 累计读写连接占用          | 42.913 s | 45.165 s | **增加 5.2%**                                |
| 已埋点完整事务累计        | 13.502 s | 13.342 s | 仅 terminal/Trace/admission/approval 四类    |
| 相同 8,830 次 commit 累计 |  2.039 s |  1.893 s | 次数相同，不能据此称 DB 总体提速             |

DB 等待为 `sqlite.wait + sqlite.read_wait + storage.queue_wait + storage.worker_wait`，不重复加入覆盖整次操作的 span；
连接占用为 `sqlite.hold + sqlite.read_hold`。SQLite 内部 busy 等待仍在连接占用/事务段中，不能把该合计当成所有数据库阻塞。
commit 次数 terminal 1,542、Trace 5,626、Turn admission 1,578、approval 84，两轮完全一致。
部分最大值继续上升：writer queue **32.957 → 77.827 ms**、reader wait **4.544 → 20.342 ms**、Turn admission 完整事务 **23.549 → 69.860 ms**。

本次数据支持“外层排队减少，同时数据库争用增加”的观察；更高并行度及新增身份解析都可能参与其中，尚无单因素实验区分它们。
不能把总发送延迟改善写成数据库提速，也不能忽略 steady 最大值与 Stop 的回退。已测 writer 在途峰值 **20 → 19**，估算保留字节峰值
**514,750 → 829,472 bytes**；reader 在途峰值 **9 → 11**。writer 64 操作/64 MiB、其中控制预留 8 操作/8 MiB，以及 2 读连接/32 租借上限均未扩大。
较低在途峰值不保证较短等待，估算字节也不是 SQLite cache、结果对象或整个进程 RSS 的硬上限。

### root 公平与慢工具中的独立进展

50 节点单根场景最终 1 根 + 49 子 Turn 均持久完成，场景用时 **1.575 → 1.259 秒**；双根为 2 根 + 48 子 Turn，
用时 **1.537 → 1.241 秒**。双根各接纳 1 个根请求，回执单次观察分别为第三轮 **7.236 / 32.378 ms**、第四轮 **14.959 / 15.028 ms**。
不能将每 root 一个请求的结果称为长期公平分位数。单根/双根 timer 最大迟滞分别 **2.400 → 3.038 ms**、**2.329 → 3.772 ms**，
场景总用时减少并未消除调度尾部回退。

在配置为 1、10、30 秒的三个慢工具情景中，第四轮实测工具区间分别为 **1.037、9.999、30.017 秒**。
每个情景的 4 次独立审批全部在慢命令运行区间内发出，4 次成功回执及 4 个其他根 Turn 的终态均早于慢命令退出，0 无响应。
第三轮也已达到这项独立进展条件；本轮保留它，并缩短了本次样本中的审批耗时：

| 配置慢工具时长 | 第三轮审批最大值 | 第四轮审批最大值 | 样本      |
| -------------- | ---------------: | ---------------: | --------- |
| 1 秒           |         5.184 ms |         3.816 ms | 各 4 成功 |
| 10 秒          |         6.478 ms |         3.775 ms | 各 4 成功 |
| 30 秒          |         5.656 ms |         4.357 ms | 各 4 成功 |

这证明这些有限慢工具情景中存在独立进展，不能推广为所有工具、账号变更或未知全局 RPC 都不会阻塞，也不能证明任意持续输入下无饥饿。
每 lane 对已识别 root 最多一个 handler，与持久 Wake 队列同 root 最多一个 `claimed` 准备是两个不同约束；Running/WaitingForApproval 不因此变成每 root 只能一个。
矩阵没有提供连续 `logical_turn_peak` 观测值；50 的边界来自配置/源码及协议回归，不能用 Provider 连接峰值替代完整逻辑占用测量。

### 测量边界与保留风险

本轮只使用临时磁盘库、假模型和受控工具，未修改 Host/Renderer 或前端保存逻辑，也没有真实用户数据、计费模型或外发消息。
矩阵是 Core Server intake 性能，未覆盖真实 Provider 网络波动、原生点击到 paint、持续 token 流和慢消费者。
只有一次同机完整运行，普通 burst 每场景三波，低样本和系统噪声仍在；没有可靠 p99、最大吞吐或 10–30 分钟稳态结论。

RSS/线程按约 1 秒间隔采样，只覆盖 Core Server 测试进程，排除 runner、Cargo 和工具子进程；两轮分别 206/200 个资源样本。
线程样本 63 → 42 不能据此证明全进程树线程预算降低或长期无泄漏，CPU 与 RSS 本轮实际增加。

磁盘统计是各场景清理后、关闭连接前的文件大小：主库总和 **259,747,840 → 259,489,792 bytes**，
WAL 总和 **158,612,912 → 158,979,592 bytes**，单场景 WAL 样本最大均为 **7,041,112 bytes**，SHM 总和均为 1,179,648 bytes。
这些都不是累计物理写入量或运行峰值，不据此宣称写放大改善。性能夹具未覆盖生产 bootstrap 的周期检查点和完整显式存储关闭路径；
对应 WAL/关闭/备份正确性由真实磁盘回归覆盖，其生产集成尾延迟仍需另测。

未审计/unknown 资源、组织配置全局 admission、单 writer、账号撤销的跨项目 automation 批清理以及同步存储桥仍保留。
下一步必须把本次 Stop、稳态最大值和 DB 等待回退保留为验收项，而不是在第五轮收敛持久化责任时仅追踪平均或 p95 收益。
本轮结果只证明第四轮既定变更的正确性及这份负载下的收益与代价，不代表后端整体六轮优化完成。

## 验证命令与证据位置

以下为本轮 release 构建、既有 lint 排除、回归和基础检查入口。归档中的日志已经压缩，摘要重算需先解压相应日志并备有匹配二进制。复跑前应复制回归脚本到新的验证目录并核对其输出路径，避免覆盖已接受日志；
脚本会选取隔离 target 中最新可执行文件，必须重新核对当前 Cargo 产物、源码和 SHA-256，不能仅凭相同二进制文件名为新代码背书。

```sh
CARGO_TARGET_DIR=/tmp/captain-backend-baseline-20261011-target CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4 cargo test --locked --release -p mycopilot-core -p mycopilot-core-server --lib --bin core-server --no-run
CARGO_TARGET_DIR=/tmp/captain-backend-baseline-20261011-target CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4 cargo clippy --locked --release -p mycopilot-core -p mycopilot-core-server --all-targets -- -D warnings -A clippy::collapsible_match -A clippy::useless_conversion -A clippy::unnecessary_lazy_evaluations -A deprecated
python3 .cache/backend-concurrency/round4-validation-20261011/run-regressions.py
python3 .cache/backend-concurrency/round4-validation-20261011/summarize-regressions.py
pnpm check:docs
pnpm check:test-layout
pnpm test:test-infrastructure
python3 .cache/backend-concurrency/round4-validation-20261011/analyze-round4.py --self-test
git diff --check
```

回归脚本对 Rust Core 使用 `--test-threads=4 --nocapture`，Server 使用 `--test-threads=1 --nocapture`；并发正确性由各测试显式生成并同步工作者，
不是依赖多个测试之间的随机交错。脚本可按组编号选择重跑；最终摘要只接受完整 37 组及当前二进制 hash。

本轮完整矩阵由 runner 在相同实现源码上重新构建并运行：

```sh
pnpm benchmark:backend-concurrency --mode full --output-dir .cache/backend-concurrency/round4-full-20261011-r1
python3 .cache/backend-concurrency/round4-validation-20261011/analyze-round4.py .cache/backend-concurrency/round4-full-20261011-r1
```

归档位于 [第四轮原始证据](results/2026-10-11-round4/README.txt)：
[元数据](results/2026-10-11-round4/metadata.json)、[汇总](results/2026-10-11-round4/summary.json)、
[逐场景比较](results/2026-10-11-round4/comparison.csv)、[逐请求原始记录](results/2026-10-11-round4/cases.jsonl.gz)、
[资源采样](results/2026-10-11-round4/resources.jsonl)、
[最终回归计数](results/2026-10-11-round4/regressions/accepted-regression-summary.json)。
完整 stage/queue/owner 比较保存在 `comparison.json.gz`，全部归档文件由 `SHA256SUMS` 校验。
`summary.json` 是依据原始记录生成的摘要，runner 本身只输出 metadata/cases/resources 及构建/测试日志。

联合 Rust Core/Core Server release 回归构建用时约 1 分 47 秒，Clippy 约 2 分钟；runner 的独立 Server 测试构建用时约 3 分 12 秒。
由于 Cargo 测试目标/feature 统一范围不同，完整矩阵使用 `core_server-7fdb09b124fbc390`，SHA-256 为
`639efb3ecf20bd2887dd2ef0ef207e572a190de980ca695a43d9d8845181679c`，不是上表联合构建的 Server 二进制。
两者都基于最终实现代码。矩阵 HEAD 为 `e787b88d045549ea5d204f5c01f77f391b7cfc49`，tracked diff 为空，
source manifest SHA-256 为 `158a6aec7a960ab3524a125fd118a2a5cffe62de7ba73bb99799816a39a12c0d`，运行前后完全一致。
Cargo.lock SHA-256 为 `9ef6403ce4922272d44e50b1c478a97827f0362acc8c88d584b06b5236011bc6`。
不能将这些 hash 用于之后修改过的源码；运行期间未修改代码/文档，最终结果文档在矩阵结束后生成。

## 局限、兼容与下一轮门禁

1. 未审计 RPC、无法证明完整冻结 owner 的 Run、部分 workflow/配置操作仍保守全局排序；组织配置修改还取得全局 Conversation admission。
   不能把允许无关 Conversation 并行解释为所有入口完全独立。
2. 无 metadata 的新会话，只有已登记且可证明新根身份的 Preparing owner 能让 Stop 越过正在执行的准备。没有 Conversation ID，或 Stop
   首次解析早于登记时仍保留全局顺序；不能从 DTO 猜未来身份。
3. 数据库仍为单 writer。账号撤销后的 automation 结算每批 100 项，但所有批次仍占一个 Control writer 作业；大量待结算项会造成跨项目等待。
   小 registry 临界区不等于整次账号操作恒定短。
4. 同 Run 的已开始持久化/发布必须与 Stop 排序；每 lane 同 root 最多一个 handler、持久队列同 root 最多一个 `claimed` 准备是不同层次的公平约束。
   不抢占同步 handler，不保证固定毫秒的最大等待，不改变 50 个逻辑 permit 或既有 Running/WaitingForApproval 语义。
5. 同步 storage 桥、第三轮旧 guard 库存、Runtime emitter/Trace observer 的持久化归属尚未全量收敛。长快照、大型删除、恢复和批清理仍随规模增长，
   取消 async 等待者不能证明同步作业已退出。旧低层 `StorageService::delete_project` 也不具有应用层已排空成员集合的运行期协议。
6. 没有 schema 迁移。内存 Preparing 不跨重启恢复，重启依靠持久 Trace、receipt、claim 和 delivery proof；运行中精确恢复与启动全局恢复不可混用。
   回退应按完整变更和构建产物处理，不能单独去掉 token/CAS/围栏后保留放宽的并行度。
7. 第五轮先明确 durable journal/cursor、Core Server 与前端保存及发布责任，逐项收敛重复保存和同步桥；必须保留 Trace/usage/上下文/通知原子边界和错误可见性，
   不删除尚未有持久 owner 的保存。生产通知投递 lease 的排队后 claim/validate/ack 仍需自己的回归，不能套用本轮 Agent token 结论。
8. 第六轮需要相同源码/机器/负载的完整比较，以及进程崩溃、磁盘满、租约、准备取消/删除、持续 token 流、慢消费者、10–30 分钟稳态与资源总预算。
   本轮测试和短矩阵不证明原生点击到 React paint SLO、可靠 p99、最大吞吐或长期不饥饿。

第四轮实现及本轮验证完成。第五、六轮仍未完成；本轮残余同步桥与持久化职责、资源成本和尾延迟回退必须进入后续门禁，不能将本轮结论推广为全部 SLO 达标。
