---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-10-12
---

# 第六轮：综合验证、修复与未通过门槛

**本轮正确性修复、独立复核和验收证据已交付；整体性能验收未通过，不能标记六轮优化完成。** 同输入36场景的50单位短历史发送p95为755.424ms，超过500ms目标；修正收集器后的600秒mixed计划中，当前版本239.861秒触发输出总字节过载，未自然排空。1,873个成功ACK逐身份恢复通过，不能用这一正确性结果替代可用性、吞吐或SLO通过。

第六轮起点`6e7bd1cc`，实现`b1744ab9`，收集器/逐Run恢复/吞吐统计修补`0d70c623`，显式关闭诊断的夹具入口修复`d4390ac3`。文档和证据另作本地提交，可用`git log -1 -- docs/performance/backend-concurrency/round6-results.md`查询最终交付提交。未推送、未发布。证据入口为[归档说明](results/2026-10-12-round6/README.md)，验证材料在`results/2026-10-12-round6/validation/`，逐次原始测量在`results/2026-10-12-round6/runs/`；具体归档文件及变换hash以[manifest](results/2026-10-12-round6/TRANSFORM-MANIFEST.json)为准。

## 改动与兼容边界

- 普通append Turn使用独立模型历史快照，从SQL起省去历史`agent_run_json`与UI state读取。消息、完整Trace、model context、compaction和版本仍来自同一snapshot；原路径主要读取展示JSON字符串，不能笼统声称此前逐条完整JSON解码。raw/model cache共用原entry/byte总预算，窄对象只能append、不能全量重写。trace-only陈旧准备要求重试，不能降级丢失最终CAS。
- checkpoint begin在writer内检查Runtime generation、取消状态与原captured Wake token，提交前复查，采样执行前再验证lease。账户执行许可仍控制新root准入，不取消已经准入任务。未确认checkpoint共享Arc，SHA256直接接收相同序列化元组；序号、fingerprint与精确重放语义保持。
- 通知在writer排队、事务、COMMIT/ROLLBACK与Server响应组装后使用新鲜时间检查原lease；过期不返回投递权，已提交事实与原token晚到ACK语义保留。
- 审批/观察器允许同scope已完成快照先应用，再做尾随刷新；保留导航generation、live suffix和新全量快照优先级，避免持续invalidation或旧分页饿死刷新。
- 新增隔离长测、原生Electron和磁盘故障夹具；修正collector重复编码、断连后继续准入以及只核对恢复总数的问题。raw吞吐现在必须通过完整恢复、回执和transport资格，失败为null。

第六轮没有schema变更，没有扩大队列/线程/业务额度，没有降低FULL、拆事务或提前ACK。根也占原50总运行额度。确实修改Renderer数据刷新与测试Host/Renderer链路；从第一轮冻结基线人工审查的全部20个生产src文件没有产品CSS、视觉资源、布局、文案、图标、动画或入口变化，权限/审批/取消/删除规则保持。这是源码范围审查，不是全AppShell视觉截图验收。详见[frontend-handoff.md](results/2026-10-12-round6/validation/frontend-handoff.md.txt)、[frontend-scope-audit.json](results/2026-10-12-round6/validation/frontend-scope-audit.json)。

| 轮次   | 实现提交                           | 文档交付          | 责任                                               |
| ------ | ---------------------------------- | ----------------- | -------------------------------------------------- |
| 第一轮 | `374e529e`、`32aa536e`、`acae0662` | `6c421d07`        | 测量、冻结基线与并发契约                           |
| 第二轮 | `b1737f51`、`46b35ea0`、`d7b3fbaf` | `f073d86d`        | lease fencing、阻塞隔离、控制通道与终态owner       |
| 第三轮 | `354762d6`                         | `06fbdb60`        | WAL/FULL、单writer双reader、提交后通知             |
| 第四轮 | `e787b88d`                         | `df3e6f04`        | Preparing/最终CAS、资源归属与公平执行              |
| 第五轮 | `e7237ce5`                         | `6e7bd1cc`        | Core Server checkpoint所有权、Renderer重复保存收敛 |
| 第六轮 | `b1744ab9`、`0d70c623`、`d4390ac3` | 用上述git log查询 | 本页所列修复、复核与失败门禁证据                   |

## 环境、二进制与统计口径

全部使用隔离临时磁盘、合成历史、内存凭据、loopback假模型和受控工具；未访问真实用户库、计费模型或外部消息，没有停止用户软件。性能进程按序独占运行，与构建/正确性测试分开。复用隔离target，release debug=0、incremental=0、jobs=4；debug缓存计数回归不参与性能比较。真实机器/编译器、lockfile、参数、源码前后manifest与binary hash在各次runner metadata中，归档仅脱敏本机路径，不修改原cache。

| 证据对象                      | 二进制SHA256                                                       | 对应范围                                              |
| ----------------------------- | ------------------------------------------------------------------ | ----------------------------------------------------- |
| 36矩阵/当前Electron           | `99349b2fa8a57731222722a455f1f0637ac3d9cda61b1ad6f14e7ee06b3aa082` | `b1744ab9`生产实现，后续仅debug断言/文档修正          |
| 旧基线Electron                | `a71882a451aa596b88435dd432def613036d5480f54f33bbb195f787675c9522` | `acae0662`生产源码，回移测试入口                      |
| 当前mixed r2                  | `a79d93917cd199819e5f5656d49bfd1ff220d2856f1a352e575e498c5b180fb9` | 新collector/逐Run恢复，旧raw派生rate必须重算          |
| 基线mixed正式r3               | `d4caef036c88b5d67268dfc97b98b3a1fc6a700b3d9831eac9d93fbbadf4745d` | baseline-r2 binary复用，runner统一Node22              |
| 交付19项定向回归binary        | `946075d21b47a1e5b22a6171ed7ad7762580e6e1e5df391a15bda040bb309a17` | 相对r2仅测量后统计资格与合成验证，不改变负载/生产行为 |
| 最终diagnostics on/off binary | `bf222b0992317f83ec8ed7ad2fe90812dc9427cdc76ae5b9fdb2115c4b88123d` | 仅再修fixture入口接受显式0或1；同binary开关对照       |

最终开关binary的fixture SHA为`2ed3e2f6c025b6da9cfe75a7c715da9e302b47cfbd50e5c486897f05cb2f8b15`，构建来源见[delivery-final-binary-provenance.json](results/2026-10-12-round6/validation/delivery-final-binary-provenance.json)。两次开关测量使用同一source/binary，运行期间source不变；测量不是新增不同Rust正确性测试，1795/1849计数保持。

来源：[binary-source-provenance.json](results/2026-10-12-round6/validation/binary-source-provenance.json)、[delivery-binary-provenance.json](results/2026-10-12-round6/validation/delivery-binary-provenance.json)、[collector-correction-baseline-copy.json](results/2026-10-12-round6/validation/collector-correction-baseline-copy.json)及[workload-equivalence.json](results/2026-10-12-round6/validation/workload-equivalence.json)。旧生产基线为`acae06623b7ce6031b90795e730968fb36e38107`；新负载仅回移测试夹具/stdio入口，保留patch，不以current完整outbound覆盖baseline生产代码。baseline r2曾使用Node23而current使用Node22，因此该次只留诊断；正式长测比较是同`Node v22.23.1`的baseline r3与current r2，均运行期间source未变。

分位取成功raw样本的nearest-rank，错误/拒绝/无回复单列；原`metrics.scheduled_to_response`混含错误，不能当成功分布。stage分位是桶上界，嵌套/并行stage不能相加为单请求耗时。发送边界为Core Server计划投放至durable admission回执，Provider开始另列；Stop回执仅表示原取消请求接收，durable terminal另核对。n=1是单次观测，小样本p99仅为经验描述。长测历史自然增长，不能始终称“短历史”。

**不引用旧raw派生吞吐。** 即便r2 binary保留旧rate字段，也用[analyze.py](results/2026-10-12-round6/validation/analyze.py)从raw重算`throughput_eligible`与原因。不合格时completed/settled rate均null；SLO失败与rate证据无效分别判定，健康但慢的完整运行可以有实测rate，仍不是最大容量。10秒块/400次block bootstrap仅描述该次合成运行的条件区间，不能称跨机器/真实Provider可靠p99；current故障截断更不能推广为完整600秒分布。

## 正确性与独立复核

最终去重 **1,795项不同Rust测试、1,849次通过执行、0最终失败**：Core1235+Server release559+debug独占1。旧1791项/1812次，新collector18次增加3项，交付19次再增加1项；重复执行不计新测试。debug15项有14项与release重复；Core Server唯一ignored profile已显式补跑。编译器计入的subprocess helper不等于新实质故障场景。逐名去重与输入hash见[resume-test-summary.json](results/2026-10-12-round6/validation/resume-test-summary.json)。

前端129/129 unit、254/254 browser，两侧各1原生Electron夹具；最终基础设施28/28、类型/lint/format与release Clippy通过。最终Node28个subtest完整包含旧25及3项validator，历史17项重跑不再相加。Clippy保留既有四类CLI排除，没有修改项目lint规则。交付release构建success、fixture6/6、outbound13/13见[delivery-fixture-tests.log](results/2026-10-12-round6/validation/delivery-fixture-tests.log)、[delivery-outbound-tests.log](results/2026-10-12-round6/validation/delivery-outbound-tests.log)、[clippy-resume-final.log](results/2026-10-12-round6/validation/clippy-resume-final.log)、[infrastructure-resume-final.log](results/2026-10-12-round6/validation/infrastructure-resume-final.log)。

三方向独立审查覆盖runtime/准备与lease、storage/恢复、Host/Renderer。当前318个Storage源码、20个前端生产和19个前端测试文件hash匹配旧验证证据，因此复用不变源码回归。审查引出的CAS降级、COMMIT后lease、响应组装后lease问题已修复并有回归；continuous send同时断言完整展示不被旧Renderer覆盖、独立UI/metadata更新、模型内容与缓存复用。首次凭据fixture16项失败、debug旧所有权断言2次失败、FTS首轮失败及修正日志保留，不改写成一直通过。审计材料：[audit-scheduler.md.txt](results/2026-10-12-round6/validation/audit-scheduler.md.txt)、[audit-storage.md.txt](results/2026-10-12-round6/validation/audit-storage.md.txt)、[runtime-handoff.md](results/2026-10-12-round6/validation/runtime-handoff.md.txt)、[storage-handoff.md](results/2026-10-12-round6/validation/storage-handoff.md.txt)、[resume-evidence-audit.md](results/2026-10-12-round6/validation/resume-evidence-audit.md.txt)；其中早期“待运行”与旧计数是当时快照，最终以去重总表及实际日志为准。

## 36场景矩阵

[本轮原始run](results/2026-10-12-round6/runs/round6-final-full-20261012-r1/) completed/exit0、source unchanged、invalidLines0、integrityFailures空；[matrix-vs-original.json](results/2026-10-12-round6/validation/matrix-vs-original.json.gz)核对36/36工作量与实际结果相同。两侧均1608 offered RPC =1572成功+36原容量拒绝，0无回复；1420 root（1316 Completed+104 Cancelled）、158 child Completed，重开0 InProgress。

36拒绝来自50单位approval/stop各18次，均为handler前`-32001/rpc_dispatch_error/overloaded/retryable=true`。两组cleanup前各有18个未结束root；approval pre-close仍18、stop已0。不能写全矩阵自然drain或把正常关闭/重开当crash耐久验证。Provider请求1679→1679、output事件22271→22271、compaction0→0；请求bytes+15133、输出bytes+12，因此业务工作量相同不等于逐字节相同。

以下均ms；旧p95取对照`before.cases[case].operations[operation].success_scheduled_to_response`，本轮完整分位见[final-matrix-success-latencies.json](results/2026-10-12-round6/validation/final-matrix-success-latencies.json)。

| 单元 | 操作       | 成功n / offered | 拒绝或错误 / 无回复 | 原基线p95* | 本轮p50 |  本轮p95 |  本轮p99 |  本轮max |
| ---: | ---------- | --------------: | ------------------: | ---------: | ------: | -------: | -------: | -------: |
|    1 | 短历史发送 |           3 / 3 |               0 / 0 |     22.427 |  19.844 |   21.217 |   21.217 |   21.217 |
|    1 | 长历史发送 |           3 / 3 |               0 / 0 |     17.490 |  30.081 |   30.587 |   30.587 |   30.587 |
|    1 | 审批       |           1 / 1 |               0 / 0 |      2.049 |   1.269 |    1.269 |    1.269 |    1.269 |
|    1 | 停止       |           1 / 1 |               0 / 0 |      1.558 |   1.383 |    1.383 |    1.383 |    1.383 |
|    5 | 短历史发送 |         15 / 15 |               0 / 0 |    101.421 |  35.856 |   77.762 |   77.762 |   77.762 |
|    5 | 长历史发送 |         15 / 15 |               0 / 0 |    194.786 |  67.878 |   99.652 |   99.652 |   99.652 |
|    5 | 审批       |           5 / 5 |               0 / 0 |      9.205 |   3.707 |    5.557 |    5.557 |    5.557 |
|    5 | 停止       |           5 / 5 |               0 / 0 |      7.176 |   5.822 |    6.998 |    6.998 |    6.998 |
|   10 | 短历史发送 |         30 / 30 |               0 / 0 |    200.799 |  55.449 |  107.812 |  107.817 |  107.817 |
|   10 | 长历史发送 |         30 / 30 |               0 / 0 |    335.498 |  88.820 |  164.118 |  170.933 |  170.933 |
|   10 | 审批       |         10 / 10 |               0 / 0 |     24.420 |   6.713 |   13.710 |   13.710 |   13.710 |
|   10 | 停止       |         10 / 10 |               0 / 0 |     32.303 |   9.526 |   11.862 |   11.862 |   11.862 |
|   20 | 短历史发送 |         60 / 60 |               0 / 0 |    377.236 | 121.854 |  247.969 |  268.257 |  268.257 |
|   20 | 长历史发送 |         60 / 60 |               0 / 0 |    714.683 | 181.135 |  401.915 |  464.909 |  464.909 |
|   20 | 审批       |         20 / 20 |               0 / 0 |    274.230 |  16.742 |   60.206 |   64.052 |   64.052 |
|   20 | 停止       |         20 / 20 |               0 / 0 |     99.684 |  11.621 |   38.993 |   42.289 |   42.289 |
|   50 | 短历史发送 |       150 / 150 |               0 / 0 |   1002.674 | 394.077 |  755.424 |  795.807 |  795.827 |
|   50 | 长历史发送 |       150 / 150 |               0 / 0 |   2235.920 | 543.720 | 1106.118 | 1192.683 | 1192.706 |
|   50 | 审批       |         32 / 50 |              18 / 0 |    633.473 |  37.289 |  143.462 |  151.384 |  151.384 |
|   50 | 停止       |         32 / 50 |              18 / 0 |    269.299 |  24.564 |   67.916 |   69.138 |   69.138 |

*1单位旧审批/停止仅有`observed_us`，表列单次观察值；不声称稳定p95。本轮n=1及小样本p99同样保留有限样本边界。

20短send p95 247.969ms，50短755.424ms为500ms门槛1.51倍；50长1106.118ms另列。1–20成功控制样本符合250ms/1s，50成功控制样本符合500ms/2s，但拒绝和样本量须并列。60秒steady-text620次成功回执p95/p99/max为380.394/439.710/498.966ms，不能替代600秒mixed。50长准备后的独立审批13.883ms只是一条观测；1/10/30秒慢工具隔离不证明持续负载永不饥饿。

当前50短/长write execution permit等待占rpc.queue累计98.58%/99.12%，p95桶上界720.895/1048.575ms；write lane仍2槽，handler平均30.580/44.971ms。lane/dependency累计仅约2ms，不能套用旧baseline全局锁解释。admission_hold是每Conversation fence；writer queue p95 5.631ms、read wait短/长0.447/1.407ms，也不能把全部尾部归为一次SQLite commit。下一步须关联持permit期间重复读取、clone/hash/assembly及持久化工作。独立定位见[final-matrix-review.md](results/2026-10-12-round6/validation/final-matrix-review.md.txt)、[final-matrix-review-evidence.json](results/2026-10-12-round6/validation/final-matrix-review-evidence.json)。

## 原生Host / Renderer对照

两侧各30submit/10approve/10stop，50/50成功、0失败/unfinished，各40Provider请求、20 Completed+10 Cancelled。30终态正文长度/hash相同，重开正文/status/完整agentRun JSON相同；各50主动作trace唯一完整、0 omitted，各920通知。原始数据：[renderer-acceptance-baseline-final.json](results/2026-10-12-round6/validation/renderer-acceptance-baseline-final.json.gz)、[renderer-acceptance-current-final.json](results/2026-10-12-round6/validation/renderer-acceptance-current-final.json.gz)及[renderer-comparison-final.json](results/2026-10-12-round6/validation/renderer-comparison-final.json)。

| 成功回执，ms | 基线p50 / p95 / p99=max | 本轮p50 / p95 / p99=max | n/侧 |
| ------------ | ----------------------- | ----------------------- | ---: |
| submit       | 13.9 /21.4 /43.4        | 11.2 /19.6 /199.9       |   30 |
| approve      | 2.1 /8.0 /8.0           | 2.0 /3.2 /3.2           |   10 |
| stop         | 2.9 /3.2 /3.2           | 2.5 /3.0 /3.0           |   10 |

首次submit最大199.9ms的冷动作回退没有剔除；Host start span95.499ms、Renderer turn.start_rpc98.6ms，剩余回调时间不能由现有分段确证归因。Renderer全量状态保存293→0、metadata保存120→0、Host RPC636→223、请求序列化bytes10,369,347→48,046、同步序列化墙钟16.579→1.005ms。接收JSON83,443,412→83,124,387bytes、parse99.992→101.576ms，没有改善。

各111次loadConversation包含81次计时外验收读和30次生产终态对账，另有重开1次，不能全算产品成本。边界为程序DOM handler→生产hooks→真实ElectronIPC/Host→Core Server stdio/磁盘；未包含完整AppShell/Markdown、硬件点击或compositor paint，double-rAF仅表示帧机会。有限低并发夹具不能证明高并发UI SLO，Host bytes也不代表磁盘写入。

## 600秒混合负载：正式对照与输出失败

正式对照为[baseline r3](results/2026-10-12-round6/runs/round6-baseline-soak-u20-20261012-r3/)与[current r2](results/2026-10-12-round6/runs/round6-final-soak-u20-20261012-r2/)，均Node22、相同collector/20owner/301波/2s计划提供6020start、500ms全文查询、固定mixed cycle；approve/stop随真实ready响应，实际数量不同必须报告。分析为[baseline-soak-r3-analysis.json](results/2026-10-12-round6/validation/baseline-soak-r3-analysis.json.gz)、[final-soak-r2-analysis.json](results/2026-10-12-round6/validation/final-soak-r2-analysis.json.gz)；current逐身份/故障复核见[resume-soak-r2-review.json](results/2026-10-12-round6/validation/resume-soak-r2-review.json)。

| 事实                                  | baseline r3 |                 current r2 |
| ------------------------------------- | ----------: | -------------------------: |
| start offered /成功ACK                |  6020 /3027 |                 6020 /1873 |
| active_run业务错误 /dispatch overload |   2355 /638 |                     526 /0 |
| approve offered /成功                 |    992 /992 |                   457 /454 |
| stop offered /成功                    |    324 /324 |                   151 /151 |
| 全文读取offered /成功                 |  1201 /1201 |                  1201 /479 |
| 所有RPC无回执                         |           0 |                       4346 |
| 其中本地断连拒发 /已handler未答       |        0 /0 |                    4344 /2 |
| 精确ACK恢复 /identity错误             |     3027 /0 |                    1873 /0 |
| 重开Completed /Cancelled              |   2703 /324 |                  1719 /154 |
| cleanup前InProgress /重开后           |        0 /0 |                       3 /0 |
| 自然排空 /transport健康               |      是 /是 |                     否 /否 |
| analyzer实测窗口                      |    610.014s | 855.061s，含故障后等待收尾 |
| 合格完成/结算rate                     |  null /null |                 null /null |

baseline虽有回执并排空，控制/send SLO失败，raw_integrity/runtime_error_free因业务拒绝未过；current另有transport/无回复/未排空等失败。不能把任何旧raw rate称吞吐，也不能因为current收到了更低的部分成功分位就宣称完整长期体验提升。

| 成功回执 | 基线r3 n | 基线p50 / p95 / p99 / max，ms               | 当前r2 n | 当前p50 / p95 / p99 / max，ms            |
| -------- | -------: | ------------------------------------------- | -------: | ---------------------------------------- |
| start    |     3027 | 2290.797 / 9658.655 / 12324.830 / 14628.446 |     1873 | 408.498 / 1615.386 / 2087.307 / 2756.632 |
| approve  |      992 | 1494.709 / 3962.704 / 5102.839 / 6869.160   |      454 | 148.663 / 700.078 / 1186.232 / 1983.125  |
| stop     |      324 | 92.354 / 5222.958 / 7000.808 / 7279.172     |      151 | 36.758 / 206.954 / 548.098 / 848.648     |
| 全文读取 |     1201 | 3007.992 / 9402.542 / 11838.600 / 14129.150 |      479 | 68.507 / 715.979 / 1207.775 / 1586.580   |

完整独立复核见[最终长测对照](results/2026-10-12-round6/validation/resume-soak-final-comparison.md.txt)及[逐owner/原件hash](results/2026-10-12-round6/validation/resume-soak-final-comparison.json)。两轮fixture SHA为`75fc4fad46311d39942dc4bdca61125527790d2306d8919d84b44efd3e84828a`、runner SHA为`4ea7aeb1948b4a3b99dc091d811876c72010febda7ccde3ed6853eafbc8b50e3`；环境除输出路径相同，实际完成工作量仍不同。

按scheduled_us<240秒选取共同窗口、允许稍后回复：各计划2400start/480read，baseline1898start成功+502active_run；current1873成功+526active_run+1已handler未答。成功send p95分别4088.488/1615.386ms、approve1973.544/700.078ms、stop1258.419/206.954ms、read3856.447/715.979ms。current成功延迟较低，但接纳量未提高且之后失联，不能称完整长期容量提升。

current表中只是故障前收到的成功样本，baseline覆盖更长实际工作；不是同存活时长的健康效率比较。baseline Provider4019请求/926,506,659输入bytes/89,919输出events，current2327/380,697,724/56,076，二者compaction均0。10秒块bootstrap区间仅条件于相应单次有效成功样本，不能消除拒绝/故障截断造成的群体差异。

current在**239.861333s**记录`rpc.response`总bytes失败：queue7,933,685+frame3,882,503=11,816,188bytes，超过原9MiB预算；frame数143、data_bytes17,539，frame_limit/data_byte_limit/data_frame_limit/oversize_busy均false。这是正常大小响应叠加超总预算，不是超大单帧。移除旧collector重复编码后仍失败，证明增长全文读取与输出背压仍需处理，不能把所有风险归于旧fixture。

4346无回执=4344本地拒发+2已handler未答。start6020中1873成功/526错误/3621无回执（3620本地拒发）；approve457中454成功/3本地拒发；read1201中479成功/722无回执（721本地拒发）；stop151全成功。2个已handler未答为start id3468/owner12和read id3485/owner19；该start无Provider首请求、无恢复Trace，不具备durable acceptance证据，不能称已ACK后丢失。1873 ACK逐run/owner/message/terminal与恢复完全一致；在线Done1855、cleanup前3 InProgress，正常关停后收敛不等于自然drain或crash证明。

| current start计划窗口 | offered | ACK | active_run | 成功p95，ms |
| --------------------- | ------: | --: | ---------: | ----------: |
| 0–60s                 |     600 | 585 |         15 |     671.184 |
| 60–120s               |     600 | 502 |         98 |    1242.189 |
| 120–180s              |     600 | 439 |        161 |    1684.398 |
| 180–240s              |     600 | 347 |        252 |    2027.720 |

第四分钟另1个start已handler无回执，后续3620start本地拒发。分组按计划投放，不保证同分钟回执。输出关闭前接纳量下降、延迟已恶化，不能只避免最后一次超限就宣布稳态达标。具体handler内CPU/存储因果仍需profile。

旧current r1约171秒中断，原collector持Events锁重复编码/析构完整history，且失联后仍准入handler。修正使用recv_frame真实wire长度、锁外一次decode/摘要化，decode后记录receipt；失联后拒发而不伪造RPC回应，记录FailureSnapshot并逐ACK恢复。旧失败原件与[final-soak-outbound-investigation.md](results/2026-10-12-round6/validation/final-soak-outbound-investigation.md.txt)完整保留；环境不一致的baseline r2仅诊断。后续交付binary只增加测量后raw rate资格/回归及显式diagnostics=0入口支持，不改已执行r2结果，亦不假称重新完成600秒长测。

每owner进度也保留：baseline ACK146–156/owner，最后ACK599.719–603.865s，最差相邻ACK间隔13.859s；current ACK88–103/owner，最后ACK236.083–238.790s，失联前最差间隔7.690s，随后每owner181个start本地拒发。较短观测的小间隔不证明600秒公平性更好；固定投放顺序造成baseline dispatch拒绝偏斜，双方均未证明长期无饥饿。

## 诊断开销与容量边界

同一最终binary、源码和输入按on→off顺序各运行一次50owner/10波有限text工作，共500start/侧；[on原始run](results/2026-10-12-round6/runs/round6-diagnostics-on-20261012-r2/)与[off原始run](results/2026-10-12-round6/runs/round6-diagnostics-off-20261012-r2/)均exit0、source不变、transport健康、无错误/拒绝/无回执、自然drain。两侧500成功ACK=500精确恢复Completed，identity错误0；Provider500次、输出6500events/41,347,950bytes、compaction0，Provider请求bytes只差3。完整参数与raw资格核对见[开关对照](results/2026-10-12-round6/validation/diagnostics-comparison-final.json)。

| 同binary单次开关观测        |                     diagnostics on |                    diagnostics off |
| --------------------------- | ---------------------------------: | ---------------------------------: |
| 成功send n                  |                                500 |                                500 |
| send p50 /p95 /p99 /max，ms | 394.897 /773.752 /829.590 /899.432 | 384.368 /761.235 /836.714 /858.635 |
| 测量窗口，s                 |                          18.958973 |                          18.979562 |
| CPU增量，s                  |                          25.452783 |                          25.225585 |
| 采样RSS峰值，bytes          |                        120,537,088 |                        117,522,432 |
| 采样线程峰值                |                                 33 |                                 34 |
| timer lag p95 /max，ms      |                       3.657 /3.897 |                       3.864 /4.009 |
| stage /queue指标组数        |                              62 /8 |                     0 /0（未采集） |

开启诊断相对关闭，本次CPU+0.90%、采样RSS+2.57%；只有一次固定on→off顺序，无随机化/重复进程批次，无法据此给精确或稳态开销估计。off缺失stage/queue是不可用，不代表成本归零；10秒块仅2个，置信分析不足。两个p95均超过500ms，所以正确完成/排空与SLO通过不同；关闭测量没有使50单位发送达标。

两次raw rate与加固后的analyzer一致，有限完成率约26.373/26.344 Turn/s可作为这两次完整finite-run描述，**不是可持续吞吐或最大容量**，不推广到600秒mixed。50短send和20owner持续混合负载已失败，尚未建立满足全部SLO的最大容量；未测档位不推断通过。

初次off尝试因fixture入口写死诊断值1，在测试开始即拒绝0，产生0场景；[stderr](results/2026-10-12-round6/validation/diagnostics-off-initial-test.stderr.log)与[stdout](results/2026-10-12-round6/validation/diagnostics-off-initial-test.stdout.log)保留，不能计通过。入口修复提交`d4390ac3`仅接受显式0或1，重编后用同新binary完成上述两侧；[Clippy](results/2026-10-12-round6/validation/clippy-diagnostics-final.log)与Rust格式检查通过。生产行为、输入与收集器不变，未将这项夹具修正说成新的性能优化。

## 资源代价

| 同业务工作量36矩阵 |           基线 |                     本轮 |
| ------------------ | -------------: | -----------------------: |
| 累计CPU            |        67.348s |       93.420s（+38.71%） |
| 采样RSS峰值        | 305086464bytes | 334626816bytes（+9.68%） |
| 采样线程峰值       |             26 |                       48 |

短50/长50/steady20 CPU分别+47.77%/+23.85%/+67.07%。新增Core Server耐久checkpoint等工作，进程含假Provider/harness，无调用栈/per-feature CPU，不能定量归责或宣称CPU效率改善。当前短/长50 timer lag p95 3.099/2.894ms，response queue p95桶上界24.575/32.767ms、serialize p95仅10/7μs；不能用旧baseline长测几百毫秒timer lag解释此矩阵。

| 长测资源事实               |   baseline r3全case |          current r2全case |
| -------------------------- | ------------------: | ------------------------: |
| 测量窗口                   |         610.014239s | 855.061164s，含故障后等待 |
| CPU增量                    |        1263.410276s |               626.973351s |
| kernel归属写增量           | 21,272,207,360bytes |        8,328,757,248bytes |
| logical writes增量         | 10,515,018,960bytes |        6,631,821,480bytes |
| WAL采样峰值                | 0（DELETE journal） |    16,121,592bytes（WAL） |
| raw resident峰值           |    869,220,352bytes |          499,040,256bytes |
| raw physical footprint峰值 |  1,442,793,056bytes |          760,120,544bytes |
| 外部ps RSS峰值             |  1,402,159,104bytes |          574,242,816bytes |
| 外部线程峰值               |                  47 |                        41 |

上表工作量/存活时长不同，只作事实，不算健康吞吐、资源效率或写入节省。kernel归属I/O包含Core Server/假Provider/harness，不是SQLite专属、SSD/NAND物理写入或fsync耗时；writer估算payload、Host bytes、WAL长度分别报告，不能混用。基线DELETE的WAL0也没有优于WAL的意义。

共同约240秒采样累计CPU baseline413.886s/current618.467s；current后续失去正常新输入，不能把600秒时较低累计CPU当效率改善。背景全文读取实际成功响应baseline1201次/185,495,234bytes，current479次/1,100,561,316bytes；响应轨迹与Core Server耐久展示变化并存，聚合还不能唯一归因，不通过删除模型或展示事实降低数据量。

current故障前0.034401–239.366070s共240个资源样本，CPU增量615.612845s、resident峰值499,040,256bytes、footprint峰值760,120,544bytes、WAL末值16,121,592bytes、DB末值258,015,232bytes。故障后240.033849–690.037848s CPU仅增加7.524562s，因为后续请求已本地拒发，不是稳态改善。全case末DB261,328,896bytes；外部ps峰值RSS574,242,816bytes、threads41，时钟为executable-relative，不能当精确故障前峰值。WAL断连后平台期和resident下降不证明正常负载有界。stage reservation有些含执行，不能把多个阶段计数相加为业务backlog。

## 迁移、故障和回退边界

schema71、SQLite3.53.2、WAL/FULL、单writer/双READ_ONLY reader保持；第六轮没有新增migration。恢复手册过时“当前v68”按源码改为71，不扩大为全手册验收。57项migration、3项database_snapshot、10项state、13项commit_notifications包含在最终Storage回归，不是用户库抽样。

SQLite FULL回滚、钉WAL快照时Backup业务恢复、terminal提交前/后两次子进程强杀和runtime checkpoint一次强杀通过。准确强杀覆盖为2个实质测试、3个自启动子进程/边界；helper不计额外故障。10k profile为64节点/10000facts/17088连续事件/SQLite busy0、20次StorageService重开，不是20次强杀或64同时Turn。300ms是checkpoint合并窗口，不是磁盘堵塞时持久化硬deadline。

FTS首轮失败在复用reader quick_check；普通UPDATE也可最小复现，fresh READ_ONLY对同一未修理库通过，旧reader MATCH后检查也通过，SQLite3.53.2/3.51.0均观察。最终对同库fresh handle做quick_check/FK，没有rebuild/修理/放宽PRAGMA；内部原因仍未获上游确认。日志和最小复现保留。

未实测OS ENOSPC/IOERR、短写、fsync失败、断电、完整数据根恢复或旧应用端到端降级。SQLite FULL不等于文件系统ENOSPC，process kill不等于掉电。第五轮`6e7bd1cc`只是同schema/WAL/checkpoint owner的隔离回退候选，未认证最低安全版本，且会失去第六轮修复；同schema也不能任意混搭Host/Renderer/Core Server。

SQLite Backup包含可读已提交WAL，不含附件/图片、Provider continuation或OS凭据；只读snapshot拒绝需要写源恢复的hot DELETE journal，不修复原库。不能只拷运行中主库或在用户库试切journal；开发reset丢弃历史，不是回退工具。成套回退需正常停生产者、排空关闭、保存完整数据根，在隔离副本验证旧版本正文/终态/pending/lease/cursor与外部引用。本轮没有执行用户回滚。完整边界及故障原始证据见[rollback-and-faults.md](results/2026-10-12-round6/validation/rollback-and-faults.md.txt)。

## 复现与归档

在对应源码worktree使用已核验binary与新的输出目录；固定Node22可执行文件，性能测量与构建/回归按序运行。`<...>`为本地新路径占位，不是本页新增测试结果。基线回移使用[baseline-adaptation.patch](results/2026-10-12-round6/validation/baseline-adaptation.patch)、[collector-correction-baseline-copy.json](results/2026-10-12-round6/validation/collector-correction-baseline-copy.json)，不复用真实用户profile。

```sh
<node22> scripts/run-backend-concurrency-baseline.mjs --mode full --waves 3 --binary '<verified-matrix-binary>' --output-dir '<new-matrix-directory>'
<node22> scripts/run-backend-concurrency-baseline.mjs --mode soak --duration-seconds 600 --units 20 --interval-ms 2000 --diagnostics on --binary '<verified-version-binary>' --output-dir '<new-soak-directory>'
python3 docs/performance/backend-concurrency/results/2026-10-12-round6/validation/analyze.py '<decompressed-run-cases.jsonl>' '<new-analysis.json>'
CAPTAIN_CORE_ACCEPTANCE_BINARY='<verified-version-test-binary>' CAPTAIN_RENDERER_ACCEPTANCE_REPORT='<new-native-report.json>' pnpm exec vitest run --project electron-fixtures src/main/core/backendAcceptance.electron.test.ts
```

归档保留成功/失败版本、源码/fixture/binary来源及采样原件派生文本；使用新staging、显式清单、拒绝symlink/非文本/NUL、路径脱敏与原件/转换文本/压缩文件三层hash。超过64KiB的文本gzip，解压后分析；gzip不改变记录的原件hash语义，详见manifest。DB、二进制、真实凭据和已有截图不纳入提交，原cache不覆写/删除。最终文档提交由页首git log查询。

## 验收结论与剩余工作

本轮实现、正确性修复、独立复核、受控对照和失败证据交付完成；可以继续本地开发和隔离验收，不能宣称六轮性能目标全部完成或正式发布就绪。门禁详见[验收口径](acceptance.md)。

| 门槛                                | 本轮结论                                                                |
| ----------------------------------- | ----------------------------------------------------------------------- |
| 正确性/逐ACK身份恢复                | 已列回归通过；长测已ACK身份均恢复，范围如上                             |
| 50短历史send p95≤500ms              | 失败：755.424ms                                                         |
| 20owner长期控制/send与稳定提供      | 失败：current239.861s输出过载，故障前延迟已恶化                         |
| 完整真实界面高并发SLO               | 未覆盖，原生夹具仅低并发DOM handler链路                                 |
| diagnostics开销                     | 同binary单次on/off已测；500 ACK/侧均恢复，p95仍失败，不能给可靠稳态开销 |
| 最大SLO内可持续容量                 | 未建立，不以有限完成率或失败raw rate推断                                |
| OS IOERR/fsync/断电、完整旧应用回退 | 未覆盖，未因ACK恢复通过而解决                                           |

后续最小工作是关联并缩短两槽send handler内的实际服务时间，为增长全文历史响应建立经过验证的有界背压路径，再用相同输入复测50短历史、持续mixed与完整界面SLO。只修最终输出超限不足以消除已测逐分钟退化。必须保留完整模型/展示事实、最终CAS、权限/lease、必要资源顺序与可靠落盘，不靠扩队列、少接纳或提前成功达标。
