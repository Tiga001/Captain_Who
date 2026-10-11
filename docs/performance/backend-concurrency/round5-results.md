---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-10-11
---

# 第五轮实测、正确性与交付边界

本轮实现 Core Server 独立保存运行展示数据、恢复投影、Renderer 权威切换与 Host 窄读取。
最终验证、性能和提交编号在下文记录；后端整体六轮优化尚未完成。
所有权与兼容规则见 [持久化契约](round5-persistence-ownership.md)。

## 范围与源码

起点 `df3e6f04ee1a8f14c8447b34841c76e50409ccbf`。源码提交 `e7237ce5216b27201022c7625cd236f2eb78f8c6`。
本页、PROGRESS 与原始证据另作本地文档提交。未推送、未发布。

修改了 Electron Host 文件读取 DTO、RPC 路由、Renderer 保存协调、审批与停止的保存依赖、发送 metadata、
协作审批/组织树/观察器刷新和历史展示映射；没有修改视觉、样式、文案、图标、动画、布局或交互入口。
原权限、审批决定、额度 50、Provider preflight、模型能力验证、消息顺序、工作流与删除规则保持。

测试全部使用隔离临时磁盘库、合成历史、内存凭据、loopback 假模型和受控工具。
没有访问真实用户库、迁移真实数据、调用计费模型、外发消息、推送或中断运行中的应用。
复用 `/tmp/captain-backend-baseline-20261011-target`，临时夹具自行清理，已有缓存与截图保持。

## 正确性与责任切换的门禁

在移除 Renderer 运行消息权威写回之前，先编译并通过 9 项基础 Runtime 检查点测试，含实际无前端消费、
进程 SIGKILL 后重新打开磁盘库与启动恢复；早期日志归档为 `checkpoint-first`，不能加入最终测试计数。
之后加入真实 spawn_agent 子运行、取消迟到帧、保存失败、交互等待、ACK 丢失和既有完整回归。
最终结果：**43 组 Rust release 回归，1,732 项不同测试、1,739 次执行通过，0 失败，1 项原有 ignored**。
修正后的回执恢复屏障另做 20 次重复，单独计数。Renderer 主回归 183 项浏览器测试、43 项单元测试，
刷新回归 41 项浏览器测试、31 项单元测试，Host/协议回归 54 项单元测试及 1 项真实 Electron 夹具通过。
web/Node 类型检查、相关 ESLint、Rust/TS 格式、Clippy、文档、测试布局及 22 项测试基础设施检查通过。
早期恢复映射 38 项单测和 4 项刷新对比夹具另有原始记录，不与最终组重复相加。

| 边界             | 验证内容                                                                                                                                 |
| ---------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| 接纳后尚未运行   | 显式 Skill 选择与初始激活摘要随接纳事务提交；未创建 Runtime 检查点即重开，保留原选择和 revision                                          |
| 无前端运行       | 丢弃 outbound receiver 后假模型持续流式输出；定时磁盘检查点含完整已确认正文/定义/游标，Done 后重开核对终态与单一消息                     |
| 真进程崩溃       | 子进程写入确认后 SIGKILL，重开后保留该正文与展示数据，孤儿运行按原规则 Failed，不伪装 Completed                                          |
| 子 Agent         | 真实 root spawn→dispatcher→child 流式运行，无 observer；完成/取消后重开，已确认正文保留，迟到帧不复活终态                                |
| 等待交互         | 检查点先于真实 suspension 接纳；关闭前端、重开服务后恢复同一问题，重复 submission 只恢复一次，Run/assistant/call 和用量正确              |
| 保存失败         | SQL trigger 拒绝检查点 revision，普通完成与批准边界仍保存失败终态、正文、Trace 和精确一次用量；未确认的问题/审批不能公开，无真实工具作用 |
| ACK 丢失         | 真实提交后 coalescer 未收到 ACK；保留精确请求，先重放相同 fingerprint，再提交新合并序号；不重新调用模型/工具                             |
| 旧快照与晚到增量 | generation/sequence/CAS、终态和删除围栏；全量旧 conversation save 不能删掉或覆盖 Core Server 运行消息，返回实际提交数据                  |
| 并发读取         | narrow DTO 的消息/项目/Trace 来自同一快照；并发提交/删除不会拼接不同 revision；完整历史仍有界串行                                        |
| 前端操作         | 接纳绑定先于 buffered event 重放；批准/拒绝/取消必须等权威 ACK，失败或丢 ACK 保留动作身份；legacy 与未接纳输入、草稿、UI 保存保持        |
| 展示恢复         | 完整 web sources/摘要输入、read 缩略图与错误、FileChange 审批状态、Skill 安装 receipt 均按原证据恢复；不能用投影授权工具执行/文件访问    |

## 同机性能比较

### Renderer 保存与刷新

使用同一浏览器夹具的 9 个、间隔 80 ms 的流式片段及 Done，完整运行消息保存调用由
**接纳 1 + 中间 2 + 终态 1 = 4 → 0**。事件 metadata 和 pair upsert 均为 0；Provider preflight 与
start RPC 仍各 1。操作结果仍在真实被模拟的 RPC ACK 后应用；没有让 handler 提前声明完成。
这是实际 hook、受控 bridge 下的调用计数，不能当作真实 Electron 点击到 React paint 的延迟。

原安全序列化器的合成 1000 项 Trace 投影为 **877,755 UTF-8 字节**；Node/Vite SSR 60 次样本
映射 median/p95 **2.355/2.612 ms**，示意 RPC envelope 为 895,064 字节、序列化 median 1.025 ms。
这些数据说明被删除全量保存路径的成本；不等于每次真实 IPC 字节，也不能乘以 4 当成磁盘写入节省。
计时可能与构建重叠，属于辅助测量；源码/夹具 hash 和原始样本均保留。

21 次刷新失效突发中，真实审批/观察器 hook 的请求 **21 → 2**，同 scope 并发峰值 **21 → 1**。
100 个有序 durable 事件（其中 10 个审批事件）下，getTree/listEvents 仍各 100，审批列表 **100 → 10**。
40 次 resync 中，getTree **41 → 3**、并发峰值 **40 → 1**；完整恢复正确补发审批刷新 **0 → 1**，成本未隐藏。
原 token/message_delta 就不触发这些读取，不能将收益归因于取消不存在的 token 刷新。

### 真实 Electron Host 窄读取

使用真实 Electron `CoreJsonRpcClient` 和生产 Core Server 的 stdin/stdout JSON-RPC；256 条合成历史、
同一文件 proof，全量/窄查询各 20 次顺序请求，全部成功，pendingPeak 均为 1，删除后 null。
没有加载真实文件或触发模型。使用新二进制上的旧全量入口与新窄入口比较，固定先全量后窄读取。

| 20 次合计指标            |   全量历史 |    窄读取 |
| ------------------------ | ---------: | --------: |
| RPC 次数                 |         20 |        20 |
| 响应 UTF-8 字节          | 34,694,335 |     5,500 |
| 请求 UTF-8 字节          |      2,035 |     3,200 |
| Host JSON parse 同步墙钟 |  13.985 ms | 0.0331 ms |
| RPC 响应墙钟合计         |  89.441 ms |  6.320 ms |
| Host CPU user + system   |  53.619 ms |  1.355 ms |

这是一条实际 Host 文件 proof 读取路径的节省，不能推断所有 Host RPC 或生产 UI 端到端均同倍数改善。
生产二进制 SHA-256 为 `5d86b4e72e26ea35ad35a5ba685513370d3447b48a20cc6776948bdd119d4cc0`。
初跑测试 bundle 使用 ESM 导致 `__dirname` 不存在，在启动 Core Server 前失败；改为 CJS 测试 bundle 后通过，生产路径未改。

### Rust Core / Core Server 完整矩阵

同一 Apple M4（10 logical CPU / 16 GiB）、release、三轮突发、同 36 个场景；1/5/10/20/50 单元，
短/128 条历史，审批、Stop、单根/双根子树，1/10/30 秒受控慢工具及 60 秒固定输入稳态。
运行 **200.685 秒**，源码始终稳定；测试二进制 SHA-256
`d9ab78afc2f4c1e56d386dcd8b71583736f416dc78661574f90d625122cfb2e9`。

**36/36 通过**：1,608 RPC = 1,572 成功 + 36 原容量拒绝，0 无响应/意外错误。
1,420 根 Turn（1,316 Completed / 104 Cancelled）、158 子 Turn（全部 Completed）逐场景分布与第四轮一致；
重开后 0 InProgress。50 审批及 Stop 场景各 32 成功、18 拒绝，仍需 cleanup 收敛后重开核对；没有隐藏此边界。

| 指标                                 |        第四轮 |        第五轮 |
| ------------------------------------ | ------------: | ------------: |
| 50 并发短历史发送成功 p95，n=150     |    610.047 ms |    704.033 ms |
| 50 并发长历史发送成功 p95，n=150     |    968.119 ms |   1067.709 ms |
| 50 并发批准成功 p95，n=32            |    132.562 ms |    131.399 ms |
| 50 并发停止成功 p95，n=32            |     70.179 ms |     70.456 ms |
| 50 长历史准备后的独立批准，各 n=1    |      8.261 ms |      8.733 ms |
| 稳态成功响应 p95，n=620              |    283.371 ms |    394.160 ms |
| 稳态成功响应最大值                   |   1277.824 ms |    497.099 ms |
| 稳态 timer 最大迟滞                  |    238.897 ms |      3.528 ms |
| 稳态完成速率，固定输入               | 10.266 Turn/s | 10.222 Turn/s |
| 全矩阵 CPU user + system             |      76.019 s |      92.606 s |
| 采样 RSS 峰值                        |   321.906 MiB |   332.219 MiB |
| 采样线程峰值                         |            42 |            43 |
| 累计已埋点 DB 等待                   |      93.442 s |     111.009 s |
| writer 排队累计                      |      83.345 s |      87.741 s |
| reader 等待累计                      |       9.833 s |      23.100 s |
| 连接累计占用                         |      45.165 s |      58.523 s |
| writer reservation 次数              |        74,981 |        78,756 |
| writer reservation 估算 payload 字节 |   895,711,966 | 1,394,852,570 |

1/10/30 秒慢工具期间，其他四个根的批准响应和终态均在慢工具退出前完成，原隔离能力保持。
批准/停止基本持平，稳态最大值和 timer 尾值改善；**短/长发送 p95 与稳态 p95 回退，CPU +21.8%、RSS +3.2%**。
不能据此声称 Core Server 或整条产品链整体更快。线程配置没有增加，采样峰值受运行时实际创建情况影响。
每个 revision 只有一轮矩阵，成功 owner 不保证逐项相同；小样本、单次观察及固定输入速率不证明统计显著性、p99 或最大吞吐。

writer reservation 字节是实际队列埋点累计的**估算逻辑 payload 预算**（约 +55.7%），包括新检查点工作，
既不是 SQLite 实际改写字节，也不是物理磁盘写入。对之前未持久化的无前端展示数据补齐保存有成本；
本轮删除前端的重复全量写回，但此 仅运行 Core Server 的矩阵没有旧 Renderer 的写回负载，不能相减推导全栈净写入节省。

矩阵边界是 Core Server 计划接收请求到 RPC 回执；不含真实前端点击、Electron UI 往返或 React paint。
相同输入/内容/Run 数量不意味着底层写入量不变：原无前端基线没有完整流式展示持久化，本轮新增了此项必要工作。
成功回执、拒绝、执行终态和重开结果分别统计；没有扩大队列或业务额度来改变成功分母。

## 中间失败与修复

- SQL 故障注入发现检查点失败转换会丢分段用量和正文。现按已暂存标记保留精确一次用量与完整 Trace，正文纳入原失败终态事务，错误独立保留。
- 实际子 Agent 取消测试发现原终态空正文覆盖已确认检查点。现只对精确 Core Server owner 在同终态事务保留已确认正文；legacy、显式正文和 completed 空正文不受影响。
- 同步人类问题先接纳再发 Waiting 状态。屏障已移到实际 suspend 入口，事务使用已确认正文；成功 pending/suspension 后封存旧段普通投影，finishReason 提前传递，避免晚到 State/Done 保存失败推翻已公开等待。
- read 失败可以合法没有 display path；校验与原 TS 展示算法对齐，不改变打开文件的授权检查。
- 2 个交错查询夹具原在同线程持 read lease 再调用 writer，触发原嵌套防护。改为独立 scoped writer thread，防护保持。
- 完整存储回归发现 FileChange/Skill overlay 增加两次无条件读取。FileChange 现按已有 builtin apply_patch Trace 证据调用；有 Trace 会话仍增加一条必要 Skill 批量读取。固定 SELECT 明确 **15 → 16**，保留历史长度独立性与 workflow 单次 Trace/来源读取断言。
- 扩展 release 测试的旧夹具未显式配置凭据后端，触发生产存储防护。测试改为显式内存凭据；未放宽防护或行为断言。
- 最终审阅发现显式 Skill 选择原先依赖 accepted 后的 Renderer 保存；移除后会扩大到 Runtime 排队与首个检查点之间。改为在既有接纳事务保存安全初始投影，不增加 writer 轮次；未把权限/秘密配置放入展示字段。
- 最终扩展回归暴露旧回执恢复测试的调度竞态：第四轮已移除生产尾部的全局 admission 锁，旧测试仍持该锁后立即 try_recv，因而不能保证恢复尚未完成。改用现有 recovery worker 的显式 gate，继续验证精确 owner 释放、completion worker 可用、Done 等待回执恢复与 107 个保留槽；未恢复全局锁或放松断言。
- Clippy 发现复杂返回类型和等价条件分支，分别以类型别名与合并条件解决；没有新增 lint 豁免。

上述失败、编译中间态不并入最终通过数，也不进入性能矩阵样本。

## 局限与第六轮前置条件

- 流式设计窗口仍为 **300 ms**。这是调度间隔，不是 writer 拥塞/磁盘失败时的硬上限；尚未确认部分在崩溃时仍可丢失，确认后必须可恢复。子观察器由原仅内存提升为同样窗口。
- 检查点按变化字段/条目合并，避免每 token 排队和反复携带完整 Trace；SQLite 仍需要读取、验证并序列化完整安全 agentRun JSON，当前未结束的正文也随检查点复制。不是常量写入成本。
- 既有 maintenance 执行预算、writer/reader 数量不变；持久化字节超限或长期失败不能通过丢弃未确认请求、截断内容或扩大队列解决。保留错误与原终态失败机制。
- 批准/同步提问的少量短事务屏障仍在同步 Runtime callback，Trace observer 与通知批次仍有原同步桥；没有宣称全链路已全部异步化。
- Done 仍需权威完整历史读取以恢复有界事件缓存溢出；同会话后续发送保留这一依赖。用户 metadata、legacy 消息与未接纳失败仍有必要保存。
- Provider preflight 本来每次发送一次，未作跨账号/权限/配置/revision 缓存。Host 通用 JSON-RPC 传输没有足够证据需改造，未添加缓存、扩大传输队列或并行放开大量大响应。
- 本轮比较了保存 RPC/逻辑序列化字节与实测传输字节，**没有可归因的操作系统物理磁盘累计写入字节**；WAL 文件末尾大小不作为该指标。没有完整生产 UI 点击到绘制的 SLO 结论。
- 第六轮可以执行：正确性和责任切换前置条件已满足。应优先针对新增检查点/安全 JSON、reader 等待、发送 p95 与 CPU 回退做剖析，再执行长稳态、磁盘满、租约和完整 Electron/Renderer 端到端门禁；本轮没有宣称全栈性能验收通过。

## 复现命令与证据

Release 构建复用隔离 target，使用 `CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0`。

```sh
CARGO_TARGET_DIR=/tmp/captain-backend-baseline-20261011-target CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0 cargo test --locked --release -p mycopilot-core -p mycopilot-core-server --lib --bin core-server --no-run
CARGO_TARGET_DIR=/tmp/captain-backend-baseline-20261011-target CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0 cargo clippy --locked --release -p mycopilot-core -p mycopilot-core-server --all-targets -- -D warnings -A clippy::collapsible_match -A clippy::useless_conversion -A clippy::unnecessary_lazy_evaluations -A deprecated
python3 .cache/backend-concurrency/round5-validation-20261011/run-regressions.py
python3 .cache/backend-concurrency/round5-validation-20261011/summarize-regressions.py
node scripts/run-backend-concurrency-baseline.mjs --mode full --waves 3 --binary /tmp/captain-backend-baseline-20261011-target/release/deps/core_server-f7a97b10b9960ccf --output-dir .cache/backend-concurrency/round5-full-20261011-r1
python3 docs/performance/backend-concurrency/results/2026-10-11-round4/compare-results.py .cache/backend-concurrency/round5-full-20261011-r1 --baseline docs/performance/backend-concurrency/results/2026-10-11-round4 --output-prefix .cache/backend-concurrency/round5-validation-20261011/comparison-final
CAPTAIN_CORE_TEST_BINARY=/tmp/captain-backend-baseline-20261011-target/release/core-server CAPTAIN_HOST_READ_REPORT=.cache/backend-concurrency/round5-validation-20261011/host-read-results.json pnpm exec vitest run --project electron-fixtures src/main/core/readToolContext.electron.test.ts
pnpm typecheck:node
pnpm exec tsc --noEmit -p tsconfig.web.json --composite false
pnpm check:docs
pnpm check:test-layout
pnpm test:test-infrastructure
```

Rust 二进制名依 Cargo feature/build fingerprint 而异，应从本次 `--no-run` 输出选择，不按修改时间猜测。
回归脚本保存每组命令、二进制 SHA、非零通过数，并以测试全名去重。最终原始证据、分组过滤器、
浏览器/Host 复现命令、统计脚本及 SHA256SUMS 位于 [第五轮证据目录](results/2026-10-11-round5/summary.json)。
四项 Clippy 命令行排除沿用前轮的既有 lint/trace API 兼容边界，未扩大。
