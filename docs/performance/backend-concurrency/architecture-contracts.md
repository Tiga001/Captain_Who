---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-10-18
---

# 后端并发优化：资源、事务与恢复契约

本文是六轮后端优化的当前实现与持续验收契约，2026-10-18 对照 `5a31c635` 核验。第一轮起点为
`aac5fa6b`，历史测量仍以各轮结果原件为准；[PROGRESS.md](./PROGRESS.md) 汇总交付，
[第六轮结果](round6-results.md)记录整体性能验收未通过。后续 writer 推进修复不替代完整复测。
既有架构真源仍为 [Core Server](../../architecture/core-server.md)、
[存储生命周期](../../architecture/storage-and-data-lifecycle.md) 和
[多 Agent](../../architecture/multi-agent.md)，本文只维护本次迁移的差异和门槛。

## 1. 不变范围与目标

前端视觉、文案、布局、图标、动画、交互入口和业务规则保持不变。权限、审批、模型与工具能力、运行额度、
消息顺序、组织/工作流、归档/删除和历史恢复规则不因性能改造改变。必要的数据调用、保存和订阅调整必须保持
用户可观察的功能语义。禁止通过扩大上限、无界线程、提前宣称成功、减少处理内容或降低落盘可靠性取得收益。

当前采用单 Core Server、各 Run 独立异步执行、按资源保留必要顺序、有界阻塞执行，以及短事务单写通道和
小型读取池。不同 Conversation 不因同属一个 Project 就自动串行。单 writer、有限 handler 槽、同步兼容
入口及大型原子操作仍会产生等待；架构已接线不表示达到延迟、长期背压或容量目标。

## 2. 已有实现必须保留

| 当前实现                                                                             | 代码真源                                                                                                                                                                                                                                                                                                               | 后续改造不能退化的性质                                                                                   |
| ------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| 独立 Approval/Control 通道；审批仅绕过明确审计过的其他 Conversation 写入             | [bounded_dispatch.rs](../../../crates/core-server/src/transport/bounded_dispatch.rs)、[审批通道测试](../../../crates/core-server/src/transport/bounded_dispatch_approval_tests.rs)                                                                                                                                     | 同一 Conversation 的 Run 别名保持顺序；未知归属和全局写保留保守围栏；关停不遗弃已开始的副作用            |
| 普通命令审批只 CAS 生命周期，不重写冻结 checkpoint；前序 Action 依赖使用索引身份查询 | [transitions.rs](../../../crates/core/src/storage/service/pending_actions/transitions.rs)、[pending_action_repository.rs](../../../crates/core/src/storage/pending_action_repository.rs)                                                                                                                               | 精确身份、唯一 dispatch 授权、通知结算与 tree-stop 检查仍在同一事务；不重新解码其他 Agent 的私有恢复材料 |
| 历史快照校验及热历史复用                                                             | [history_snapshot.rs](../../../crates/core/src/storage/service/history_snapshot.rs)                                                                                                                                                                                                                                    | 不能跨数据库实例/会话 epoch 复用；历史变更仍在 admission 最终 CAS 检查                                   |
| Trace 可信后缀增量发布和终态围栏                                                     | [trace_publication.rs](../../../crates/core/src/storage/service/trace_publication.rs)                                                                                                                                                                                                                                  | 实例身份、journal revision、不可变前缀验证；提交失败失效游标；成功提交后才推进游标                       |
| World State 模型投影与 ledger 复用                                                   | [world_state.rs](../../../crates/core/src/context/frame/world_state.rs)、[assembler.rs](../../../crates/core/src/context/assembler.rs)                                                                                                                                                                                 | 复用不能绕过来源、epoch、模型观察边界和不可变历史校验                                                    |
| durable Turn occupancy、树停止围栏、文件副作用结算                                   | [occupancy.rs](../../../crates/core-server/src/application/agent/turn_executor/occupancy.rs)、[cancellation.rs](../../../crates/core-server/src/application/agent/run_lifecycle/cancellation.rs)、[destructive_mutations.rs](../../../crates/core-server/src/application/agent/run_lifecycle/destructive_mutations.rs) | 重启不能重复启动同一 Turn；停止/删除不能把未知外部结果当成已撤销                                         |

## 3. 资源归属与排序

下表是当前资源契约，不是 DTO 字段清单，也不授权移除未知归属或未审计路径的全局围栏。

| 资源范围                    | 权威身份来源                                                   | 需要协调的操作                                                               | 不应被该范围阻塞的工作                                                           |
| --------------------------- | -------------------------------------------------------------- | ---------------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| Conversation                | Core Server registry 与持久 Conversation/Trace                 | 新 Turn、编辑重发、模型迁移、压缩、恢复、删除；同一对话只能有合法的当前 Turn | 其他 Conversation 的普通发送、审批和停止                                         |
| Run                         | registry 与 Trace 的 Run → Conversation/assistant 绑定         | steering、停止、终态、恢复、审批延续                                         | 其他 Run 的模型等待和工具执行                                                    |
| Action                      | durable pending action 的 action/run/call/message 身份         | approve/reject/cancel、dispatch claim、执行回执                              | 其他对话的全量消息保存和历史读取                                                 |
| AgentTree(root, generation) | 持久 root 身份、树停止记录、Run/claim 绑定                     | spawn/followup、claim/admission、根停止及其后代清理                          | 不相关 root 的调度；同项目的独立人类对话                                         |
| Project 删除                | Core Server deletion lifecycle 标记、成员关系和存储复核        | 阻止新副作用登记；取消、排空、最终删除                                       | 其他 Project；未删除 Project 内不同 Conversation 的普通执行不需要独占 Project 锁 |
| Account 授权 epoch/revision | Main 下发后由 Core Server 持有的 process-only execution access | 新人类 Turn 的权限变更与最终 admission 校验                                  | 长准备和模型等待不能一直持有全局授权锁                                           |
| 真正全局                    | Core Server 生命周期和数据库实例                               | 进程关停、实例/schema 迁移、账号授权版本发布                                 | 不能把未经审计的方法简单归入任意 Conversation 后放行                             |

DTO 中的 `conversationId`/`runId` 仅为定位和排序提示。transport 必须通过 Core Server 权威状态解析 owner；handler
仍执行授权、精确身份和最终 CAS。未知 owner 不能被解释为“无冲突”；沿用保守围栏，直到明确审计方法的效果范围。
修改 transport 分类必须同时添加错归属、跨会话、同会话不同 Run 和全局操作测试。

多资源操作应在执行前解析并声明完整集合，再按稳定类别和规范化身份排序获取范围，或交给统一协调器；禁止执行
到一半从 Conversation 动态升级成 Project/Tree/global 独占。删除 Project 的成员快照若已变化，最终事务必须
复核或拒绝重试，不能漏掉新成员。

### 锁边界迁移记录

第一轮时 [turn.rs](../../../crates/core-server/src/application/agent/turn.rs) 在准备和接纳期间持有全局
`conversation_admission`，新人类 Turn 同时持有 `execution_access`。仅拆前一个锁仍不能让准备真正独立。
取消和删除也获取 admission；删除还可能等待文件副作用结算。另一方面，
[launch.rs](../../../crates/core-server/src/application/agent/turn_executor/launch.rs) 在 Run 结算后恢复组织 claim
时借用同一 admission 锁，防止把“已 claim、未写初始 Trace”的准备窗口误判成孤儿。

第四轮已将上述长 admission 改为按 Conversation 的逻辑预约，权限更新与最终接纳在 writer 边界协调，完成尾段只恢复精确 Run。
Preparing、按 Run 发布、项目及其实际成员删除、RPC 归属重解析与保留的全局围栏见
[第四轮资源调度契约](round4-resource-scheduling.md)。下列约束继续适用：

1. 排序/owner registry 的锁只用于取得快照、安装 reservation 或 generation，尽快释放。
2. 资源范围内安装 Preparing/删除/取消令牌后，外部准备移到锁外；网络、文件复制、压缩、模型调用和工具执行不得
   占用全局 admission、数据库 connection 或写事务。
3. 最终状态推进在短范围协调和完整领域事务中完成，复核 reservation、历史/配置 revision、树 generation、
   删除标记、claim 和权限。不能从陈旧读取缓存授予执行权。
4. 数据库 writer 不回调一个正在等待 writer 的 owner/权限锁；权限撤销与最终提交必须选择并测试单一线性化顺序。
   不能在缺少等价原子性证明时把现有 `execution_access_guard` 简单换成不受保护的预检查。
5. 不持有全局生命周期锁等待文件副作用 drain；先安装范围标记，使新 effect 注册与删除互斥，再锁外等待，最后短
   事务删除。旧 worker 必须受 tombstone/generation 约束，不能重建已删除资源。

实现由 [Preparing registry](../../../crates/core-server/src/application/agent/preparing.rs)、
[keyed admission](../../../crates/core-server/src/application/agent/keyed_admission.rs) 和 bounded dispatch 协调；
以上边界不意味着所有同步路径已迁移，也不是整个仓库的完整锁顺序证明。

## 4. Preparing、最终 CAS 与执行生命周期

当前主生命周期：

```text
收到请求 → 校验身份/容量 → 安装 Preparing(token, generation)
                            ↓
                    锁外、可取消的有界准备
                            ↓
                 最终校验 + 原子 durable admission
                            ↓
                Running ↔ 合法的等待/审批/恢复
                            ↓
                 原子终态 + 回执 + 释放占用
```

`Preparing` 是进程内逻辑状态，不是 schema 新枚举。token 绑定 boot ID、generation、Conversation/Run；
持有它的任务已经受 RPC/执行器容量约束。重启丢弃 token，并以 durable trigger claim 和初始 Trace 判断是否已接纳；
不能把内存 reservation 当跨重启 authority。只有启动恢复可作孤儿 claim 扫描，普通完成只恢复精确 Run。

最终 admission 至少复核：Conversation/Project 存在与归属、允许的归档/删除状态、历史 revision、当前 Run 占用、
模型迁移/压缩边界、Preparing token/generation、树停止、源消息/claim 身份、账号授权 epoch/revision 和有效期。
现有 [turn_writes.rs](../../../crates/core/src/storage/service/conversations/turn_writes.rs) 已把相应 Turn 消息、
初始 Trace、自动化/子 Agent 绑定等放入领域事务；新实现要保留完整原子边界。

同一 Conversation 的两个启动只能一个成功；失败准备只能释放自己的 reservation。取消/删除/权限撤销先推进
generation 或持久围栏；晚到的准备结果不能重新启动。已提交 admission 但响应丢失时，应按稳定请求和 Run 身份恢复，
不能重复建消息或盲目重放外部副作用。

长准备不能直接占住按 Conversation 串行的 actor 消息处理器。应提交准备作业后归还控制权，结果携带 token 返回，
这样该 Conversation 的停止和撤销也能及时生效。

## 5. claim、租约、恢复与容量

第一轮时 [agent_dispatcher.rs](../../../crates/core-server/src/application/agent_dispatcher.rs) 在 `run_claimed_wake`
中同步调用 `executor.start`，随后才进入包含续租 tick 的 observe/settle。将 start 移到线程池时，排队和准备时间
会成为明确的租约风险；不能只加 `spawn_blocking` 而忽略 claim。

第二轮已在 claim 前取得物理准备容量，并从 claim 起独立续租；第四轮另限制同一 root 的未接纳准备。两者共同覆盖排队/准备全过程，且必须继续保留竞态测试。最终 commit 再次
验证 claim token/有效状态，失去租约的 worker 不得执行或结算另一 owner 的工作。续租、停止和取消不能被普通准备
队列耗尽；共享无限 blocking pool 不是隔离。

恢复只能依据已提交 Trace、claim/receipt、树停止和当前 preparation ownership。A 的完成触发恢复时，不能释放仍在
准备的 B 的 claim。重启后无法证明已执行的外部动作必须保持 outcome unknown/indeterminate，不承诺外部非幂等
系统的 exactly-once。

保持现有逻辑 Turn 容量定义，特别是等待审批期间保留的 permit 及等待交互的既有规则。调度池执行槽与业务 Run 上限
分开计量；本次不得偷改“等待审批是否占额度”。队列必须同时约束数量和字节并记录最老等待。公平性按 Conversation
及 root 观察，不能让一个大树永久饿死其他 root。

## 6. 存储事务与通知

第一轮时 [StorageState](../../../crates/core/src/storage/mod.rs) 是一个 `Mutex<Connection>`，打开时设置 busy timeout，
未显式选择 WAL/synchronous。不要把开发数据库模式当生产默认；基准每次记录实际 PRAGMA 和 SQLite 运行时版本。

第三轮已迁移到专属 writer、两条只读连接和显式 WAL/FULL；详见 [存储执行契约](round3-storage-execution.md)。

当前一个专属阻塞 writer 执行已迁移的完整写操作，两条只读连接负责已审计查询；兼容 guard 共享相同写 gate 和连接。
写预算 64 项/64 MiB 估算载荷（控制预留 8 项/8 MiB），读租借最多 32 个在途；它们不是 RSS 或产品容量承诺。
禁止直接把整个 StorageService 方法塞进 writer：方法内部可能有附件复制、解压或其它慢计算。
读方法必须按真实效果审计，`get/list/poll` 命名不构成只读证明。多查询快照在同一个短 read transaction 中捕获数据和
cursor，释放事务后再解压/序列化；权威授权和执行决策仍走最终写事务复核。

必须保持为完整事务的边界：

- Turn admission：消息身份、初始 Trace、历史 revision、claim/自动化绑定与相应授权。
- 正常终态：助手正文、Trace/model-context、用量、通知、相关组织投递结算与 grant 撤销。
- 审批：精确 Action 的状态 CAS、tree-stop 检查、dispatch 授权与通知结算。
- 组织投递：输入/事件/幂等回执，及 claim/bind/settlement 的原有原子关系。

当前 `update_hook` 仅标记受影响 journal dirty。操作退出且 autocommit 恢复后核对实际已提交头，头推进才通知；
rollback、savepoint 回滚或 COMMIT 失败不产生假成功。一次操作先提交后回滚时保留前一提交的通知；核对失败保留 dirty，
不能把已提交业务改报失败。真源见 [commit_notifications.rs](../../../crates/core/src/storage/commit_notifications.rs)。
watch 只是加速信号，durable journal sequence/cursor 才是事实；重连、漏信号、同批合并和回退 catch-up 必须保留。
当前存在回退轮询，不能把上述风险表述成必然永久丢事件。

Trace cursor 的 instance identity 属于整个数据库实例，不能在增加连接后变成“每个连接独立身份”。只有成功 commit
才推进 trusted cursor，任何事务失败或外部修订不一致仍需失效并完整校验。

WAL 已启用，当前依赖锁定 bundled SQLite 3.53.2，磁盘库验证 WAL/FULL 并设置 1000 页自动 checkpoint，
Core Server 每 30 秒执行有界 PASSIVE。长 reader、SQLite FULL、备份与进程强杀的已测边界见第六轮报告；
这些不构成 OS IOERR/fsync/真实断电认证。禁止为改善延迟默默降低 synchronous。备份继续用
[database_snapshot.rs](../../../crates/core/src/storage/database_snapshot.rs) 的 SQLite backup API，不能只复制主文件。

`5a31c635` 修复异步写推进：首次 poll 注册拥有输入/permit 的任务，FIFO 前驱释放后直接提交就绪任务，不依赖
awaiting future 再次 poll。提交前取消撤票；提交路径取得后 writer 拥有真实完成与 drain，不能靠取消等待者撤销 SQL。
磁盘测试覆盖无 repoll 的写入/周期 WAL、迟注册、取消析构重入、竞争及 panic 排空；这不是第六轮性能重验。

## 7. 文件、持久化所有权与关停

### 文件准备

[fork 路径](../../../crates/core/src/storage/service/conversations/projects_and_history.rs) 已先形成冻结历史/旁路记录计划，
释放读快照后 clone continuation、复制/fsync 并以不覆盖方式发布文件；最后 writer 事务复核 source identity/revision、
side-journal 摘要、权限/模型/额度/停止及幂等回执，再发布引用；失败只清理自身文件，结果不确定时保留恢复证据。
文件系统与 SQLite 不是联合 ACID，须接受可回收的
孤儿文件而避免已提交行指向未发布文件。不要从少数 fsync 路径推断所有现有附件都已具备断电零损失保证。

### Core Server 与 Renderer

当前 Core Server 拥有流式正文和安全展示 checkpoint，按 300 ms 窗口合并，最多保留一个最新投影和一个未确认请求。
generation 安装在 writer 内校验当前 segment 与适用 lease，进入 Runtime 前再查 lease；后续补丁复核精确 Run、终态与 generation/sequence/fingerprint。丢 ACK 先重试同一请求，确认后才推进。
审批/同步交互公开前有保存屏障，终态仍提交 Trace/model-context/usage/通知等原原子事实。详见
[第五轮所有权契约](round5-persistence-ownership.md)。

Renderer 确认精确 Core Server owner 后省去全量运行保存和无关前置 flush；legacy/未接纳消息及草稿、metadata、UI 保存保留，
失败保留原 payload 并在显式 flush/退出时传播。子 Agent observer 同样不成为磁盘 owner。
300 ms 是调度窗口，不是落盘 SLA；展示 checkpoint 不授权崩溃后重放模型，恢复仍需合法 pending/suspension。

每个 RPC 区分“收到”“入队”“持久接纳”“开始执行”“执行结束”；优化不改变既有返回字段含义。
Main [JSON-RPC client](../../../src/main/core/jsonRpcClient.ts) 已可同时挂起多个 request ID；没有全局逐请求等待响应。
其同步序列化、stdin backpressure 和响应处理要分别测量，不能用“Host 完全串行”解释所有延迟。

### Shutdown

保持 [bounded dispatch](../../../crates/core-server/src/transport/bounded_dispatch.rs) 当前语义：显式 shutdown 停止
新 admission、未开始的 handler 返回明确结果，已开始的 blocking mutation 必须等待或纳入可恢复结算，不能 abort
JoinHandle 后宣称没有提交；EOF 与显式 shutdown 的排空语义按现有协议保留。

关闭 claim 和新 effect 登记后取消运行、持续处理必要续租/结算、drain 阻塞作业与有归属的 Provider transition 后台任务。
业务生产者停止后再停周期 checkpoint、drain 存储、关闭读池/TRUNCATE/writer；关闭失败返回错误。超时必须明确未完成和未知结果，
不把“发出取消信号”等同于外部进程退出或持久终态。恢复仍从 durable facts 进行，不依赖 Renderer 在退出前保存成功。

## 8. 实施顺序与当前验收状态

| 轮次         | 可做的改造                                            | 合入前置条件                                                                      |
| ------------ | ----------------------------------------------------- | --------------------------------------------------------------------------------- |
| 2 阻塞隔离   | 有界 preparation/控制执行、租约与关停调度             | 记录排队/执行差异；准备阶段 claim 协议、shutdown 与控制响应竞态                   |
| 3 存储架构   | 单写通道、小读取池、锁外文件准备                      | after-commit 通知先落地；patched SQLite/可靠性核验；原子事务与 read snapshot 门禁 |
| 4 多对话调度 | owner scope、Preparing/CAS、缩小全局锁                | 2/3 对应边界已就绪；取消/删除/撤销/claim recovery 竞态通过                        |
| 5 保存链解耦 | Core Server checkpoint parity、Renderer/Host 调用调整 | Core Server 全生命周期恢复证据；终态与旧 checkpoint 围栏、RPC 语义不变            |
| 6 综合验收   | 固定负载对比、故障、长期运行与独立审查                | 前五轮门禁与可回退方案；不能以仅提高吞吐掩盖尾延迟或正确性回归                    |

上表保留迁移的依赖顺序，第 2–5 轮实现已落地；第 6 轮证据已归档，但整体性能验收失败。50 单元短历史发送未达目标、
长测 outbound overload、资源代价和完整 UI SLO 缺口必须继续列为未完成项，不能用局部正确性通过替代。
回滚代码与回滚数据库 schema 是两件事；任何 DDL 改动都需单独迁移/备份/兼容检查，不以重置用户数据作为回退方案。
对应验证映射见 [acceptance.md](./acceptance.md)。
