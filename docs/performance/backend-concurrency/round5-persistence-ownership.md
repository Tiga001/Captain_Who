---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-10-11
---

# 第五轮：运行数据所有权与恢复契约

起点 `df3e6f04`。本页描述第五轮的实现边界；通过的验证、实测与尚未通过的门禁以
`round5-results.md` 和 `PROGRESS.md` 为准。第一至四轮已建立的单 Core Server、额度 50、短事务单 writer、
两条只读连接、按资源排序、Trace journal、提交后通知与终态原子事务继续复用。

## 数据生成、保存和恢复

| 数据                                               | 生成者                            | 普通对话持久所有者 / 恢复来源                                                                                                                   | 子 Agent观察器                                                                  | 协作审批投影                                                                            |
| -------------------------------------------------- | --------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| 用户输入、附件引用、消息顺序                       | 用户 / Host；子任务由协作调度生成 | Core Server 接纳事务原子写 user/assistant 对、Trace 和必要上下文；Renderer 未接纳的本地失败消息保留独立保存路径                                 | child admission 保存真实输入；observer 只读                                     | 不通过审批卡片创建用户输入                                                              |
| 未发送草稿、用户标题/项目/模型选择、折叠等 UI 状态 | Renderer                          | 原 metadata、draft、UI 专用 API；失败保留待保存内容                                                                                             | 观察器不能修改子对话                                                            | 不从审批刷新覆盖用户状态                                                                |
| 流式正文、当前流 previousContent 和未锚定时间线    | Runtime                           | Core Server 每 300 ms 合并检查点；reset 精确恢复前一正文，Trace 接管已提交旁白                                                                  | 复用同一 Runtime 检查点；内存 observer stream 仍负责实时 cursor，不是磁盘所有者 | token 不触发审批列表刷新                                                                |
| 工具定义、版本、todo、响应时间、finishReason       | Runtime                           | Core Server 安全展示补丁；不把 terminal State/Done 当作已经提交的终态                                                                           | 同上                                                                            | 不由审批列表反写                                                                        |
| agentRun、Trace、工具调用/结果、旁白和错误顺序     | Core Server Runtime / Host        | 原 Trace 增量 publication/cursor，历史读取在同一快照按 Trace 重建；checkpoint 不重复序列化完整 Trace                                            | exact Run/assistant 身份相同                                                    | pending/audit 只投影对应 action，不重新执行                                             |
| 网页/文件读取展示辅助数据                          | 工具 / 现有展示算法               | Core Server 保留经过白名单校验的展示输入；历史与 Trace builtin 身份、callId、tool、result 对应后合并；Renderer 复用现有 normalize/Markdown 算法 | observer 走同一 history mapper                                                  | 与授权无关，不能用展示数据授权打开文件                                                  |
| 文件修改、Skill 安装、MCP、命令 Session            | 各自 Core Server 执行服务         | 独立持久记录、pending/audit/Session/Trace 的同快照投影；安全 checkpoint 补充运行期展示                                                          | 只读投影                                                                        | 外部执行和决策仍由原 action 身份/状态机约束                                             |
| 显式 Skill 选择、动态激活                          | 用户 / 模型                       | 接纳事务保存用户 id/revision 与激活摘要；resume 不把模型后续激活混入显式选择；动态激活从 Trace 恢复                                             | 同上                                                                            | 安装批准不等于自动改变选择                                                              |
| 模型上下文、Provider continuation                  | Core Server                       | 原 journal、provider vault、revision/fence；不从 Renderer message JSON 还原模型权威                                                             | 独立 child context                                                              | approval checkpoint 恢复原配置和权限                                                    |
| usage                                              | Core Server 分段结算              | waiting segment 与终态现有事务；普通展示补丁不能覆盖                                                                                            | child usage 原归属不变                                                          | 批准快于老段退出也不能漏算                                                              |
| pending action、批准/拒绝/取消                     | Core Server / 用户决策 RPC        | 原 pending/action audit 和事务；公开审批前先保存当前展示检查点，原冻结权限与最终校验保留                                                        | 真实子 Run 仍是执行主体                                                         | 祖先可见卡片只是投影；ACK 后更新界面，失败保留卡片                                      |
| 等待交互、回答                                     | Core Server 人机交互服务 / 用户   | 原 suspended checkpoint、response/admission、恢复协议；运行展示不是回答权威                                                                     | 原准入规则不变                                                                  | 不把交互回答合并成普通展示保存                                                          |
| 完成/失败/取消终态                                 | Core Server                       | 正文、Trace、模型上下文、用量、必要通知沿用原原子提交；Done 必须等待它成功                                                                      | 同一事务和 Wake fencing                                                         | 后续展示更新不能重新开启或改写终态                                                      |
| 通知及游标                                         | Core Server journal/observer      | durable event sequence 提交后发布；renderer 重连按快照/游标恢复                                                                                 | observer generation/sequence 仍区分快照与实时流                                 | 只在 approval_projected/approval_updated 失效审批列表；完整重载另递增 hydrationRevision |

模型活动动画、重试倒计时、工具参数预览、未提交文件 diff 预览、命令输出预览和 finalAnswerReady
仍是临时展示状态，沿用原规则不进入消息持久投影。完整文件/原图沿原授权读取入口按需加载。

## 检查点、确认及恢复

`coreCheckpoint` 是安全 agentRun 投影里的可选 `{ generation, sequence, fingerprint }`，无需数据库迁移。
generation 是每个实际 Runtime segment 的 UUID；sequence 是 JS 安全整数；fingerprint 由 Storage 从
精确 Conversation/assistant/Run 身份、generation、序号、正文和补丁计算 SHA-256。它不是授权凭据。
只有 Core Server 内部 begin 能安装它；Renderer 不得给新消息伪造该字段，也不能覆盖已有 Core Server 所有者。

begin 在每个 Run 的发布保护内检查当前 cancellation token，事务再检查精确 InProgress Trace。
apply 在 writer 事务内检查 generation 和 expected sequence；相同请求重试返回 Replayed，
相同 sequence 不同内容或跳号返回错误，旧 generation、已删除/重写/终态消息返回 Retired。
未确认请求始终保留，先重试完全相同的请求，确认后才能提交合并的新状态。重试只写内部幂等检查点，
不会重复模型调用、工具执行或用户决定。

每个 Runtime 至多保留一个最新投影和一个未确认请求，没有逐 token 队列。普通增量每 300 ms 经既有
maintenance 有界执行器进入单 writer；没有新增线程。只提交变化的展示字段，Trace 仍只追加新后缀。
待批准/同步等待用户输入在实际 pending/suspension 接纳前保存展示检查点；等待事务接纳后封存旧段 coalescer，
后续 State/Done 不增加会推翻已公开等待的普通保存屏障。finishReason 通过内部字段提前交给屏障，公开事件协议不变。
模型段返回后先停止 timer 并完成检查点，再进入原终态事务。
有限的同步审批屏障仍是残余同步桥，不能宣称所有 SQL 都已从 Runtime callback 异步化。

显式 Skill 选择随 user/assistant 和初始 Trace 的接纳事务提交，覆盖接纳 ACK 后、Runtime 尚未开始的崩溃边界。
普通 root 的完整内存执行配置不因此成为展示数据；没有合法 pending/suspension 时，崩溃重启按原规则收敛失败，
不会改用当前 Provider/权限偷偷继续执行。模型 ID 已由原 admission 保存，待批准/同步问题仍从冻结 resume input 恢复。

原普通 Renderer 也是固定 300 ms 窗口；本轮不扩大设计窗口，且前端关闭后仍工作。
子观察器原先的未结束流仅在内存中，本轮增加同样窗口。300 ms 是调度窗口，不是磁盘拥塞下的硬上限；
writer 阻塞或保存错误会延长未确认窗口，错误保留诊断并在下一周期/显式屏障重试。
token 通知不表示持久化接受。进程崩溃可能失去尚未提交的窗口，已确认检查点必须重开可读；
startup 对无活动 owner、无合法 pending/suspension 的 Trace 按原规则收敛失败/取消，不能伪造 completed。

Renderer 展示仍实时消费事件。保存错误不再删除待保存队列并假装 flush 成功；metadata、upsert、
UI 保存同样保留内容。显式 flush 和退出握手传播错误；失败不无限紧循环重试。
Core Server 终态之后的迟到旧 Renderer 快照受所有者/终态围栏保护，UI 专用保存继续可用。
取消或待批准的空输出只对精确 Core Server checkpoint owner 保留已确认正文，legacy 与显式非空输出保持原规则；
失败检查点的正文进入原失败终态事务，错误仍独立保留，已暂存分段用量不重复累计。
同步提问的事务使用已确认正文，写入失败不公开请求；回答沿原 submission identity 幂等接纳。
普通显式 Stop 发出取消信号后，Runtime 退出仍先补刷当前 coalescer，再提交取消终态；首个 300 ms 之前的增量也走此屏障。
Stop RPC 本身保持“停止请求已接收”的语义，不是正文或终态持久化 ACK；其后立即进程崩溃仍只保证已确认窗口。
基线 Renderer Stop 的 flush 同样为异步触发，未等待完成。启动恢复、没有活 owner 或关机超时强制收敛只保证最后确认的内容。

## 调用链与兼容

文件打开只需要某条 assistant/run/call 的证明时，Host 使用 `storage.loadReadToolFileContext`。
它在同一 SQLite 读取快照返回项目、消息存在性、Run、typed 工具身份、路径和最小结果。
不排到 `queuedConversationReads` 的全历史响应后，也不放开大量全历史并发；旧全历史读取队列保持。
不存在消息、已删除、legacy proof、观察器活动 proof 的原有规则均保留。

协作审批和观察器刷新采用每 scope 一个活动请求加一个尾随请求；导航/重载有 generation fencing，
错误和重连不被陈旧请求覆盖。组织树 catch-up 和完整 hydrate 共用读取通道，完整恢复优先。
token 原先就不刷新组织树/审批列表，收益不能归因于删除不存在的 token 请求。

发送前现有 provider preflight 每次发送只有一次；未发现可跨模型、权限、账号或 history revision 安全复用的
重复调用，因此保持。Core Server admission 保留最终身份、权限、版本和模型能力校验。
只有与接纳事务重复的乐观 metadata 保存可去重；此前用户真实 metadata 改动仍需按序提交。

FileChange 恢复仅在已读取 Trace 含 builtin apply_patch 时执行；Skill 安装外置审批在有 Trace 的会话增加一条批量查询。
普通带 Trace 历史固定 SELECT 从 15 到 16，与消息条数无关；这项正确性成本计入实测。

第一轮及本轮未发现必须改造 Host JSON-RPC 传输的独立瓶颈；不新增 transport 缓存或扩大队列。
RPC 的接收、持久化接纳、开始运行、执行完成保持区分。前端视觉、文案、布局、动画、交互入口及业务规则不改。
