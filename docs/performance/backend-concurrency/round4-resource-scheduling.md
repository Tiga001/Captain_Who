---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-10-11
---

# 第四轮：资源排序、Preparing 与恢复边界

本页记录第四轮代码的实现契约和验证范围，不作为性能验收结果。第三轮已接受的存储架构与结果仍见
[存储执行契约](round3-storage-execution.md)、[第三轮结果](round3-results.md)；本轮的最终源码状态、回归日志与完整负载比较由
[PROGRESS](PROGRESS.md) 和最终结果材料记录。下文不声称完整矩阵已运行、所有路径已无全局阻塞，或任一延迟指标已改善。

## 1. 分开的三种所有权

| 所有权                | 作用                                                                            | 释放条件                                                   |
| --------------------- | ------------------------------------------------------------------------------- | ---------------------------------------------------------- |
| RPC/物理执行预算      | 限制队列数量、估算字节及同步工作线程；按已解析资源/root 调度                    | 实际 handler/同步作业退出；等待者取消不代表同步作业停止    |
| Preparing reservation | 在初始持久化 Trace 之前记录本进程正在准备的 Conversation、Run 和 assistant 身份 | 对应 `PreparingGuard` 实际退出；取消只发出意图，不提前释放 |
| 逻辑 Turn 占用        | 保留既有全局 permit、Conversation 当前 Turn 和持久化 Trace 语义                 | 既有 Turn/审批/等待/终态规则所定义的释放边界               |

Preparing 不是新的业务 Run 额度，也不是数据库 schema 中的新运行状态。准备记录受既有已接纳作业所有权约束；
初始消息、Trace、claim/receipt 与绑定的领域事务仍是跨重启接纳事实。数据库 writer、逻辑 Turn 额度与物理线程预算不能互相替代。

## 2. 资源从 Core 身份解析

[rpc_resources.rs](../../../crates/core-server/src/application/agent/rpc_resources.rs) 以持久化 Conversation、Agent/root、pending Action、
question 及 Core 的 Run/Preparing 注册表解析排序范围。DTO 里的 ID 只是查找提示，不能单独授予并行执行或修改权限；handler
仍执行授权、精确归属、revision、claim 和最终事务检查。解析失败、无法确定 owner、未经审计的配置/全局方法采用保守全局排序。

范围分别包含 Conversation、Run、Action/question、树及 Project 成员/删除标记。普通 Project 成员之间不互斥；Project 删除标记
与其成员冲突。删除根树、迁移等归属变化先满足相关前序依赖，后续请求再解析新归属，不能永久缓存旧 owner。
概览读是明确审计过的快照观察，包括无会话归属的 Office 状态探测；先前概览读取不阻塞后继控制或写入，但后继概览仍等待先前全局写。特定 Conversation 的读保留相应的先写后读顺序，未知方法不因名字像读取而获得并行特例。

审批从持久化 pending Action 的冻结输入中读取原 Project，并与该 Conversation 当前 metadata 的 Project **同时**声明。
Stop/steer 从 Preparing、active Run、usage context 和 pending Action 收集冻结 owner；这些 owner 必须指向同一 Conversation，
所有原 Project 与当前 Project 都进入集合。这样移动 Conversation 以后，对旧 Project 的文件作用域与新 Project 的成员删除仍相互排序。
仅能从 Trace/session 找到 Conversation、不能恢复冻结 Project 的 Run 继续使用全局围栏。迁移本身还解析并声明已存在的目标 Project。

已审计的新 Turn 方法在 Conversation 尚不存在时使用 `GlobalTurnPreparation`：它保持普通操作的全局冲突，只允许账号发布按
第 4 节规则越过；不能把任意未知 owner 的 RPC 都当成可绕过的准备。身份解析在独立有界 worker 上执行，只复制小型 owner 查找提示，
不为解析再次复制完整消息或 checkpoint。owner 变化后的重解析与依赖等待均发生在取得 handler 槽之前。

Transport 的资源声明只负责排序。它不允许在 handler 中跳过授权，也不允许执行中临时从一个 Conversation 升级为全局独占。
完整多资源集合必须先声明；[KeyedAdmission](../../../crates/core-server/src/application/agent/keyed_admission.rs) 对集合排序、去重，
原子预约全部键，拒绝同线程嵌套取得同一个 coordinator，避免部分持有和升级死锁。

## 3. Preparing 生命周期

[preparing.rs](../../../crates/core-server/src/application/agent/preparing.rs) 的 token 包含进程随机 `boot_id`、单调 generation、
Conversation 和 Run。注册项还保存权威 Project、assistant ID、历史 revision、阶段及取消令牌。

```text
解析/授权 + Conversation 预约
    → Preparing 注册
    → 有界历史、模型兼容与输入准备
    → writer 内复核 token、权限、revision、claim 等条件
    → 原子初始 Trace / 消息 / 绑定提交
    → Running 或持久化后的既有失败结算
```

同一 Conversation 仅一个 Preparing owner；相同 Run 不能同时登记到两个 Conversation。不同 Conversation 可以同时准备。
`mark_committing` 只是阶段标记，不赋予绕过取消的权利；真正有效性仍在有序 writer 边界检查。旧 token、旧 boot 和迟到 guard
不能验证或删除后继 generation。已进入 Committing 的项不能改写捕获的历史 revision。

运行期间取消和删除通过与最终接纳相同的 writer 顺序发布围栏，避免“最后检查通过后、提交前撤销”的窗口：

- Stop 可按 Core 已登记的 Preparing Run 找到 owner，设置该 generation 的取消令牌。尚无初始 Trace 时，不制造一个已接纳 Turn。
- 删除先登记 Conversation/Project tombstone 并取消匹配项，再在 registry 锁之外等待真实准备作业与相关副作用退出。
- 失败删除可撤除 tombstone，已取消的旧 generation 不会因此恢复有效。
- 关机发布停止标记并取消 Preparing；旧作业仍保留物理预算与所有权，直到实际退出或调用方明确报告排空超时。

Project 删除先围栏 Project、等待本地已登记的 Conversation 修改，再在 writer 中捕获包含已归档和子 Conversation 的全部成员，
同时登记这些 Conversation 的围栏。排空同时覆盖 Project 和这组 Conversation，因此成员移动后仍能等待冻结在旧 Project 下的文件作用。
应用把已围栏并排空的完整成员集合交给
[`delete_project_checked_members`](../../../crates/core/src/storage/service/conversations/projects_and_history.rs)。writer 内、普通删除及
Agent 树删除的最终事务内均复核成员集合；新增、移入、移出或移除成员都会使旧集合失效，要求重试，不能把未排空的新成员顺便级联删除。
集合比较不依赖输入顺序和重复项，附件文件清理只在事务成功后进行。这个最终 CAS 是应用层排空协议的一部分；旧的直接存储
`delete_project` 入口仍不接收排空集合，不能据此声称所有低层调用都具有相同的运行期保证。

Conversation 的逻辑预约可以跨越准备，但 coordinator 的全局 bookkeeping mutex 不跨越文件、SQL、模型或工具工作。
service 层 Stop/删除使用自己的控制围栏，不取得该 Conversation 的长准备预约；transport 仍保留已声明资源的排序，只有经过
Core Preparing owner 证明的 Stop 可以越过已经开始的匹配 Turn 准备，不能越过未知全局请求或尚未开始的 Turn。

## 4. 账号许可发布和排队请求

[execution_access.rs](../../../crates/core-server/src/application/agent/execution_access.rs) 冻结 `identity_epoch` 与 `denial_generation`。
同账号连续有效续租不使准备失效；拒绝、退出、更换账号或已观察到过期会永久使旧 token 失效。重新登录不能把先前未接纳的请求
重新解释为新账号授权。期限还使用单调经过时间和已观察时钟跳变校验，不能只比较一次墙钟。

权限发布和最终接纳都经过
[execution_access_boundary.rs](../../../crates/core/src/storage/service/execution_access_boundary.rs) 所用的 writer 顺序。
账号 registry 只在检查/替换状态时短暂持有，不持锁等待 writer，也不在持锁时执行其他存储方法。最终接纳在 writer 内复核冻结 token。
既有运行、人工回答重放和恢复是否需要新许可仍由原语义决定，不把所有恢复都当作新人类 Turn。

生产 [request loop](../../../crates/core-server/src/transport/request_loop.rs) 与性能 harness 使用的
[baseline dispatcher](../../../crates/core-server/src/transport/baseline_dispatch.rs) 在 start/continue/rewrite Turn 和人工回答请求入队前
捕获许可 token **或拒绝结果**，handler 在 `with_queued_execution_access` 作用域内运行。线程局部栈按 service 的共享许可状态身份
匹配，RAII 在返回或 unwind 时弹出；它不跟随 `AgentService` clone 进入后台续跑。请求排队期间退出再登录也不会取得新账号许可。

账号发布走独立 Control lane 和 storage Control writer 接纳，不等待待接纳 Turn 的完成。transport 单独公布已审计的 preparation
类型，先于完整 owner scope：即使第二个准备还因第一个准备而等归属解析，撤销也不会经这条间接依赖被卡住。handler 最终仍检查
入队 token，未经审计的全局方法仍保持屏障。service 撤销回归、排队 transport 竞态、生产/harness 共用接线需要分别核验。

拒绝许可后，尚未接纳的 automation attempt 按每批 100 项事务结算。批次间不持有账号 registry，但当前实现仍在**同一个 Control
writer 作业**中循环清理，可能使其他数据库写入等待；不能把短 registry 等同账号操作已完全局部化。任一批失败保留拒绝状态并返回错误，
不会恢复之前的 grant 或提前确认持久结算成功。

## 5. Run 发布与审批的锁范围

原先跨 SQL/事件发布持有的进程级取消、pending Action 注册表改为短快照/投影更新。较长的工作按资源 gate 排序：

| coordinator                           | 实际键与目的                                               | 允许等待的工作                         |
| ------------------------------------- | ---------------------------------------------------------- | -------------------------------------- |
| `conversation_admission`              | Conversation；新 Turn、模型迁移、压缩等相互排序            | 同一 Conversation 的准备和有序提交     |
| `action_admission`                    | Run；同一 Run 的审批决定和 predecessor 推进                | 冻结审批内容的校验、决定及其结算       |
| `run_publication`                     | Run；问题/Waiting/审批事件与 Stop、generation 替换相互排序 | 该 Run 的持久化和事件入队              |
| cancellation / pending registry mutex | owner/token/投影快照                                       | 本轮迁移路径仅短内存操作               |
| lifecycle activity                    | 冻结 Project/Conversation/Run 的活动计数                   | 终态持久化、快照及对应发布，供删除排空 |

需要两个 gate 的路径使用 `action_admission → run_publication → 短 registry`。
取消敏感事件在 Run gate 内确认 token 仍与当前执行 generation 共享状态，再发布事件；过期 emitter 不能取消后继 Action。
若需要从 emitter 调用内部 Action 取消，先释放 Run gate。register/unregister 与 Stop 也使用相同 Run gate；
按 token 退休的旧 worker 不能删掉后继取消令牌。

[MCP 批量失效](../../../crates/core-server/src/application/agent.rs) 先快照受影响 Run 集合、排序去重，一次取得全部
`action_admission` Run 键，再重新读取待处理项；若等待期间出现集合外的新匹配 Run，释放全部键后重新发现，不持部分键升级。
持久结算后只删除身份及原状态都仍匹配的内存投影，registry 不跨 SQL；取消 runtime token 时再取得对应 Run 的 publication gate。
pending 的持久人工决定凭证仍保留，失效的是原执行 payload；已批准或执行中的项按既有终态规则结算。

[组织发布](../../../crates/core-server/src/application/agent/workflow_execution.rs) 的 send、mail mutation 和组织修改提交在 Run publication
gate 内复核当前 segment token，随后执行持久事务。不同 Run 不因共享 cancellation registry 等待另一 Run 的 SQL，Stop 与同 Run 的
已开始发布仍有顺序。组织配置修改继续使用全局 Conversation admission；此次拆锁没有把所有组织/配置操作变成独立资源。

关机首先发布全局停止标记，再逐 Run 取得发布 gate 并取消当时的当前 token。等待发布结束时可能发生 generation 替换，
因此不能只取消先前全局快照中的 token。晚到的注册在同一 gate/短 registry 内检查关机标记并立即取消新 token，
同时仍登记它以保留结算/排空责任。

这些 gate 保证资源之间的独立性，不承诺同一 Run 的 Stop 可以越过已开始的持久化/事件发布。数据库仍是单 writer，已有同步
存储桥和其他未迁移路径仍见 [第三轮库存](round3-legacy-storage-inventory.md)，不能把此次改动描述为所有全局锁都已消失。
root terminal 与 action continuation 通过 `register_lifecycle_activity` 在短删除 registry 锁内检查范围并增加活动计数，返回的
`FileEffectGuard` 保留排空责任，不保留 mutex。删除在 writer 中登记标记后等待既有活动，后到活动被标记拒绝；长 SQL 和事件工作
因而不需要持删除 registry 等待 writer。Control writer 内登记围栏与终态排空仍是独立验收门禁，不能只删 mutex 而丢失运行期保护。

## 6. 完成期恢复不能扫描活跃准备

[workflow_execution_repository/delivery.rs](../../../crates/core/src/storage/workflow_execution_repository/delivery.rs) 区分两种恢复：

- `recover_run(run_id)` 只查看该 Run 的持久 Trace；只有确知已终态时才结算其输入回执。无 Trace、仍 InProgress 均不推断失败。
- `recover_claims()` 是启动时、任何活跃准备尚未接纳之前的全局孤儿扫描。此时缺少 Trace 的遗留 claim 才可按既有失败恢复规则处理。

Runtime 的完成尾段调用精确 Run 恢复，不再借全局 Conversation admission 锁保护一次全局扫描。A 已完成时，B 可以处于
“邮件/input 已 claim，但 assistant 与初始 Trace 尚未提交”的合法窗口；A 的 Done 和恢复不得把 B 误判为孤儿。
重开数据库后仍依赖持久化 delivery proof、Trace 和 receipt，不恢复内存 Preparing token。

## 7. 公平与容量边界

生产 [bounded dispatcher](../../../crates/core-server/src/transport/bounded_dispatch.rs) 的预算如下；接纳数量包含排队与执行中的请求：

| 预算                    | 第三轮   | 第四轮 | 接纳数量 / 估算字节                                 |
| ----------------------- | -------- | ------ | --------------------------------------------------- |
| Read handler 槽         | 4        | 4      | 64 / 64 MiB                                         |
| 普通 Write handler 槽   | 1        | 2      | 64 / 64 MiB                                         |
| Control 专属 worker     | 2        | 2      | 32 / 16 MiB                                         |
| Approval 专属 worker    | 2        | 2      | 32 / 16 MiB                                         |
| 共享身份解析专属 worker | 无独立池 | 2      | 队列上限为四 lane 接纳数量之和，另有 1 MiB 估算预算 |
| 全局逻辑运行额度        | 50       | 50     | 既有业务 permit 规则                                |

Read/Write handler 仍使用默认 blocking pool；槽数不是额外创建的专属线程数。解析任务已经持有原 lane 的数量/字节 permit，
不会因为切到解析池就免除接纳预算。新增 2 个解析 worker 与增加 1 个普通写槽必须计入线程、CPU、RSS 和排队成本分析；
本轮没有扩大业务额度或 RPC 接纳队列。逻辑 50 见
[`DEFAULT_AGENT_GLOBAL_CONCURRENCY`](../../../crates/core-server/src/application/agent_dispatcher.rs)，不能用它代表全部物理线程预算。

KeyedAdmission 对冲突请求 FIFO；不相关键可越过被阻塞的请求，已排队的全局屏障不能被后来者不断超越。
多键预约不提前占有子集，guard 退出/异常展开时释放原集合，短期资源不留下永久 per-key mutex 表。

Transport 的 [ready 队列](../../../crates/core-server/src/transport/bounded_dispatch_resources.rs) 按 Core 解析的 root 轮转：Agent 使用图中
`root_conversation_id`，普通 Conversation 使用自身 ID，Project 删除使用该 Project 的独立调度标识。每个 lane 内，同一已识别 root
同时最多一个 handler；这不是跨 lane 的全进程互斥，也不是将一个 root 的逻辑运行数限制为 1。资源依赖等待不占物理 handler 槽。
队列项目仍计入既有数量和字节预算，关闭和回执语义不因公平调度而放宽。公平调度不抢占已经运行的同步作业，不保证任意输入下
固定毫秒的最大等待。

子 Agent 的 [公平 claim 查询](../../../crates/core/src/storage/agent_graph_repository/wakes.rs) 在同一 writer 事务内跳过已经拥有
`claimed` 准备的 root，检查和 claim 在同库的多个 Host 间保持原子性。生产 dispatcher 使用该新入口；旧 FIFO 存储入口仍保留。
一个 root 的先前 Wake 已进入 Running 或
WaitingForApproval 时，其兄弟仍可按原规则接纳；这限制准备阶段的调度占用，不改变逻辑 Turn 总额度、审批等待额度或同 Agent
单个活跃 Wake 的原有约束。该行为需要公平 claim 单测、dispatcher 回归和双 root 完整负载共同验证。

## 8. 回归设计与验收限制

新增测试分别位于 [Preparing 单测](../../../crates/core-server/src/application/agent/preparing/tests.rs)、
[资源预约单测](../../../crates/core-server/src/application/agent/keyed_admission/tests.rs)、
[service 准备集成测试](../../../crates/core-server/src/application/agent/turn_executor/resource_preparation_tests.rs)，
以及 transport、审批隔离、删除/账号、fair claim 和 workflow repository 对应测试模块。
Project 成员集合的真实磁盘回归位于
[checked_project_membership.rs](../../../crates/core/src/storage/service/tests/conversations/checked_project_membership.rs)，覆盖普通删除与 Agent 树删除分支；
拒绝前后通过独立只读连接比较全部应用表，随后以新完整集合成功删除并重开核对。

service 集成测试只使用临时磁盘数据库、内存 credential store 和本机假模型。同步 channel/hook 固定竞态窗口，检查不同对话能否
同时到达准备边界、同一对话是否只接纳一对消息/Trace、Stop/删除能否先发布围栏、旧账号与旧历史/模型快照能否被拒绝，
以及 A 的真实完成尾段是否保留 B 的真实 Preparing claim。另有 Run 发布阻塞隔离与关机后继 generation 退休验证。
终态回归在真实模型完成后暂停已登记的持久化活动，验证删除 writer 能发布标记、无关磁盘写入可前进、活动未退休时删除超时拒绝，
释放活动后真实终态保存/发布完成，再次删除成功；暂停点同时检查全局 registry mutex 已释放。
它们是正确性回归，不是吞吐、CPU、RSS 或调度尾延迟测量。

编译/测试通过数量、必要的历史回归、完整矩阵、同处理量比较与原始日志必须另列；未获得结果前不在本页记为通过。
本轮未改变 Electron Host、Renderer、页面/交互、业务额度、权限规则、审批语义和业务 schema。

## 9. 尚存边界与交付门禁

- 未审计方法、找不到完整 owner 的 Run、部分 workflow/配置操作继续采用全局 transport 排序；组织配置修改仍取全局 Conversation admission，账号发布仍与普通写入共享单 writer。
- 无 metadata 的新会话，仅在精确 Preparing owner 已登记且可证明新根身份时允许 Stop 越过其已开始准备。完全未提供 Conversation ID，或 Stop 初次解析早于 Preparing 登记时仍保留全局顺序；不会从任意 DTO 推定未来 owner。
- storage 同步桥、旧 guard 库存及同步 Runtime emitter/Trace observer 尚未全量迁移；本轮不得标为全后端异步化或无全局长工作。
- 账号拒绝的 automation 清理仍有跨项目循环；同 Run 的持久化/事件排序、同 lane 的 root 限制也可能产生真实等待。
- 无抢占的同步 handler、长 SQLite 快照、文件/工具排空与关机超时仍需单独测量；取消等待者不能被记作已停止实际作业。
- 固定输入完成速率不是最大吞吐；低样本 p99、原生点击到 paint、持续 token 流、慢消费者、崩溃/磁盘满和 10–30 分钟稳态不能由定向回归代替。
