---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-10-11
---

# Host 与 Renderer 测量边界（第一轮）

本轮只增加默认关闭的诊断；不改变队列容量、界面、审批决定、持久化顺序、RPC 返回值语义或原有保存等待。这里的数字日志是供后续固定负载采集的测量入口，并非已经完成真实桌面端到端压力测试。

## 启用与采集

- Host/Core Server：启动隔离测试实例时设置 `CAPTAIN_PERFORMANCE_DIAGNOSTICS=1`。
- Renderer：Vite 启动或构建时设置 `VITE_CAPTAIN_PERFORMANCE_DIAGNOSTICS=1`。这是构建环境开关，不能仅在已经打包的程序进程上设置后期待生效。
- 不为采集重启正在运行的用户软件，不对用户真实数据库施压。
- Host 输出 `[core-performance]`；Renderer 控制台输出 `[renderer-performance]`。使用现有测试窗口控制台收集器保存日志对象为 JSON，勿解析成纯自然语言后丢掉分布。
- 聚合窗口为 30 秒。Host 仅在 Core Server 子进程存在且诊断启用时，每秒采样事件循环迟滞和队列状态；子进程结束清理定时器。Renderer 有活动时才启动窗口定时器；未完成动作持续报告 pending。
- 所有跨度均使用本进程 `performance.now()`；日志明确标记 clock。禁止直接相减 Renderer、Host、Core Server 的单调时钟值。

## 指标语义

两个进程共用固定边界直方图（`src/shared/latencyHistogram.ts`），保存固定数量的桶，不保留无限增长的原始样本。

- `p50UpperMs`、`p95UpperMs`、`p99UpperMs` 是所在桶的上界，不能写成精确百分位。
- `bucketCounts` 比固定边界多一个元素，最后一个是超过最大边界的开放桶，并另外标为 `overflowCount`；落入开放桶的百分位以上述样本 `maxMs` 作保守上界。边界采用小于等于，非有限值和负耗时不入桶。
- `count`、`totalMs`、`maxMs` 是实际样本聚合；`bucketCounts` 与 `bucketUpperBoundsMs` 可以合并多个窗口。不能平均窗口 p95。
- 小样本仍输出分位桶供定位，但不据此宣称可靠 p99。压力验收要报告样本数、错误数和未完成数。
- 方法标签使用固定 allowlist，未知方法统一为 `other`；动作和阶段为固定代码枚举。日志不记录请求参数、返回内容、对话 ID、运行 ID、错误对象或密钥。

### Host

| 指标                     | 起点/终点及限制                                                                                                              |
| ------------------------ | ---------------------------------------------------------------------------------------------------------------------------- |
| `request.serialize`      | 构建 JSON 请求前至序列化完成；包含这期间的本地诊断登记                                                                       |
| `request.stdin_written`  | 调用 Node stdin.write 前至 write callback；是 Node/管道写入确认，不是 Core Server 接纳、执行或提交                           |
| `stdin.backpressure`     | write 返回 false 的次数，时间值为 0；不把它误读为排队时长                                                                    |
| `response`               | 本地请求登记至解析到匹配响应；包含序列化、管道、Core Server 处理、输出与解析；普通审批可能包含执行等待，不能统称 durable ACK |
| `response.parse`         | 单个响应或通知的 JSON.parse 时长与帧字节数                                                                                   |
| `notification.dispatch`  | 同步调用本地通知订阅者的时长，不包含它们启动的异步工作或 Renderer 处理                                                       |
| `event_loop.delay`       | 1 秒探针超过计划触发时刻的迟滞，不是 CPU 使用率                                                                              |
| `pendingCurrent/Peak`    | 等待响应的 Host 请求数量；不是 Core Server 队列或执行数量                                                                    |
| `stdinBytesCurrent/Peak` | Node writableLength；不是整个进程或系统管道全部缓存字节                                                                      |
| `oldestPendingMsPeak`    | 在请求变化与 1 秒探针上采样的最老未响应请求年龄峰值                                                                          |

`outcomes` 分为 `offered`、`completed`、`rpc_error`、`transport_failed`。offered 指通过本地 client 存活检查、即将编码写入 Core Server 的请求；不包括调用前的账号拦截、关闭拒绝。completed 只表示 JSON-RPC success response，不能替代不同方法的领域完成事实。Transport failure 可能是结果未知，不能作为重新执行工具的依据。诊断不新增超时或自动重试。

`traces` 最多保留每窗口前 32 个终结响应，包含现有数字 `rpcId`、静态方法、Host elapsedMs、outcome；超出记 `omittedTraces`。RPC id 只用作独立 trace 字段，不能加入指标标签。它可以与同一个 Core Server client 会话中的 JSON-RPC id 关联；不同进程/不同 client 的 id 不能直接合并。该有界首批样本不是随机尾延迟样本；全量延迟分布以直方图为准。

### Renderer

| 动作                     | 已接入阶段                                                                       |
| ------------------------ | -------------------------------------------------------------------------------- |
| 普通审批批准、拒绝、取消 | `preflush` → `rpc.wait` → `state.apply` → `total`                                |
| 协作审批                 | `rpc.wait` → `state.apply` → `projection.refresh` → 再次 `state.apply` → `total` |
| 既有对话发送             | `preflush` → `provider.preflight` → `total`（包含随后提交回调完成）              |
| startConversationTurn    | `rpc.wait` → `total`；含 Host IPC 往返，不是纯 Core Server 执行时间              |
| cancelAgentRun           | `rpc.wait` → `total`；负面决定记 skipped，不改返回值                             |

普通审批仍在 RPC 前等待 flush。协作审批原来没有该前置 flush；决定 RPC 返回后先应用权威状态，再等待原有 refresh，最后重申权威状态。两个路径应分别分析，不能把普通审批的前置等待推定给所有子 Agent审批。

`state.apply` 测量同步调用 update/setState 的开销，**不等于 React commit/paint**。`total` 从诊断接入的回调阶段开始，不包含鼠标硬件事件到回调被调度的等待。发送的顶层 span 覆盖既有会话正常提交分支；新会话创建、确认模型切换后重试、队列自动发送的前置过程未全部覆盖，但实际 start RPC 会覆盖。

每个动作有本地 `actionSequence`，Renderer 聚合中携带临时 `rendererSessionNonce`（32 个小写十六进制字符）。每窗口最多记录 64 个阶段 trace，其余只聚合并记 omittedTraces。普通审批、协作审批、start/cancel RPC 会把这两个字段作为独立可选参数传入 Host API，经 preload 的独立 IPC 参数发送；**业务 DTO 和 Core Server JSON-RPC wire 完全不添加字段**。

Host 在诊断开关打开时，检查 metadata 仅含这两个字段、nonce 长度及格式、sequence 为正安全整数。无效 metadata 被忽略，不能影响原有鉴权或业务执行。合法上下文由 AsyncLocalStorage 跨异步 await 传递，避免并行对话相互串号；CoreJsonRpcClient 将它保存在 PendingRequest，匹配结果时写入有界 `trace.renderer`，并附现有 numeric `rpcId`。因此可以按 Renderer 的 nonce+sequence 找 Host trace，再在同一 Core Server client 会话按 rpcId 找 Core Server trace。metadata 不用作任何指标标签、资源归属或权限决策。

Core Server 的首次懒启动特意退出该诊断上下文创建长寿命 streams、timer 和 onStarted 订阅，避免所有后续后台调用被错记到第一次点击。关闭诊断时不创建 ALS scope，不添加 IPC 参数，不改变原 Promise/异常路径；传输层校验仍执行原有业务 DTO 校验。

关联范围有限：前置 storage 保存和 provider preflight 仍按 Renderer span 分段计时，不逐个向内部 RPC 透传。`turn.submit` 与深层 `turn.start_rpc` 是各自的本地 span，尚未穿透整个发送协调栈传递父子关系。原生点击前调度、所有其他业务 RPC 和 React paint 不在当前逐请求关联范围。

## 验证

2026-10-11，以下验证通过（具体提交统一由 PROGRESS.md 记录）：

```sh
pnpm exec vitest run --project unit src/main/core/rpcDiagnostics.test.ts src/main/core/rpcDiagnosticContext.test.ts src/preload/AgentIpcBridge.test.ts src/main/core/jsonRpcClient.environment.test.ts src/renderer/src/app/__tests__/actionDiagnostics.test.ts src/renderer/src/app/__tests__/pendingActionDecision.test.ts src/main/core/ipc.agentCollaboration.test.ts
# 7 files / 87 tests passed
pnpm exec vitest run --project browser src/renderer/src/app/__tests__/McpActionDecisionSafety.browser.test.tsx src/renderer/src/features/agentCollaboration/__tests__/useCollaborationApprovals.browser.test.tsx
# 2 files / 15 tests passed
pnpm exec tsc --noEmit -p tsconfig.node.json --composite false
pnpm exec tsc --noEmit -p tsconfig.web.json --composite false
```

另外对本轮 Host/Renderer/shared TS 文件运行 ESLint 通过。测试覆盖固定桶尾部、分布可合并、标签/trace 有界、失败及 pending 计数、诊断关闭不分配定时器、实际 CoreJsonRpcClient 接线且不记录正文、已有审批错误安全与协作审批导航/刷新竞态。

跨 IPC 关联另验证了两动作异步交错、两个实际 client RPC 响应乱序、非法 metadata 被忽略且账号检查仍执行、DTO/wire 不添加诊断字段、禁用时 IPC 参数数量保持不变，以及普通和协作审批在浏览器内的实际透传。

追加独立复查验证了诊断开关两种状态下的 Promise identity、同步异常、相邻 continuation 顺序，以及 Host 重复 start/stop、重启、旧进程迟到 exit 的计时器清理。关闭诊断时 startConversationTurn 和提交返回保留原有 Promise 路径；测量函数以旁观 then 回调记录结算并返回原 Promise。

这些测试是测量正确性与行为回归测试；不能用其运行时长作为真实用户发送/审批 SLO 达标证据。尚未覆盖原生点击至回调排队、React commit/paint、发送协调栈内全部父子跨度和生产窗口工作负载，后续验收必须明确区分这些缺口。
