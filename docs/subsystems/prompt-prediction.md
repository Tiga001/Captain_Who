---
status: current
audience: developers
owner: engineering
last_verified: 2026-11-03
---

# 预测下一条用户消息

输入框可以在一条助手回复结束后，用当前选定的模型预测用户可能输入的一句话。它是独立的、可能产生费用的 Provider 请求，不是自动续写对话的 Agent Turn。候选先作为占位文字显示；用户按 Tab 才写入草稿，发送仍需另行确认。

## 请求从哪里来

1. 开关默认关闭。只有 `storage.setPredictionPreference` 能修改 SQLite 中的预测偏好；普通整份 UI 设置保存不能覆盖它。实际开关变化会推进偏好代次，成功设置后 Core Server 清除预测缓存，Main 再通知各窗口。启动时较晚返回的旧设置不能覆盖已经收到的变更。
2. 交互会话已加载、草稿已恢复、最后一条为有正文的成功助手回复且输入框可用时，Renderer 才考虑预测。已有输入、队列、已选 Skill、正在导入的附件或模型切换等状态会阻止展示。打开历史聊天也使用同一入口，并非只有收到实时完成事件才能触发。
3. Renderer 通过 `agent.predictNextUserMessage` 只传 `conversationId`、`assistantMessageId`、`modelId`。Main 使用与开启 Turn 相同的登录/许可检查；Core Server 捕获 execution access token，独立确认偏好已开启、用户有权写此会话、会话未归档且没有 Turn 占用、目标仍是最后一条有效回复，以及所选模型可用且满足 Provider 切换要求。只读子 Agent 观察器不是可写目标。Renderer 提供的 ID 和开关都不是模型调用授权。
4. 响应只有 `{ suggestion: string | null }`。视图仅采用仍匹配当前目标的结果；输入框聚焦且为空时，单独按 Tab 接受，不触发发送。切走只解除当前订阅；手输、粘贴或开始输入法组字会收起本次查看中该回复的候选。入口实现见[输入框](../../src/renderer/src/features/chat/components/ChatComposer.tsx)与[订阅逻辑](../../src/renderer/src/features/chat/useNextUserMessagePrediction.ts)。

协议真源：[TypeScript DTO](../../packages/protocol/src/agent/core.ts)、[Rust method 名](../../crates/protocol-rs/src/methods.rs)、[共享契约 fixture](../../packages/protocol/fixtures/agent-contract-v1.json)。Main 的[预测请求](../../src/main/ipc/agentIpc.ts)和[偏好写入/广播](../../src/main/ipc/storageIpc.ts)是两个不同入口。

## 模型实际看到什么

| 候选来源                 | 如何组装                                                                                                                                                                                                             | 何时可以使用                                                                                                                    |
| ------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| 热路径：刚完成的回复     | 当运行上下文匹配当前会话、未绑定协作身份且不是 Automation 执行时，可在最终采样时保留原请求，追加完整助手回复和专用预测指令。沿用原请求的消息前缀、工具定义和适用的 Provider 私有回放。中间采样请求在执行工具前释放。 | 先暂存在内存；只有完成终态持久化后才绑定已提交历史版本。会话、回复、模型及连接/协议版本、历史版本、偏好代次和执行授权都须匹配。 |
| 冷路径：没有可用的热材料 | 从 SQLite 的已提交模型历史恢复，以当前所选模型、允许的草稿权限和 Skill、当前能力指南与工具投影重建请求；需要时按正常 Provider 规则恢复私有回放，再追加预测指令。                                                     | 适用于重新打开应用、内存材料被淘汰或不再匹配的情况。重新检查身份与历史；不保证逐字还原上次的临时指南或命中 Provider 缓存。      |

两条路径都只在请求副本中追加指令，不创建聊天分支、不写父会话的 Trace 或上下文日志，也不推进压缩。保留工具定义是为了复用正确的请求前缀，不是授权执行；预测不会启动工具、发布聊天流事件或把结果当作用户许可。种子会清空请求的连接凭据和端点字段；实际发送前以摘要核对端点并使用所选连接的凭据。模型历史、图片及适用的私有回放仍可能随请求发往该 Provider，不能把预测描述成纯本机推理。

采样在已知上下文窗口时先检查容量；装不下就不发请求，不为预测压缩原对话。输出预算不超过 512 Token（已配置更小上限时取更小值）。单次传输限时 30 秒，不执行普通聊天的自动重试；这不是含排队和准备在内的端到端时间保证。有工具调用的响应直接丢弃；否则只接受单字段 JSON 中的非空、至多 200 个字符且不含控制字符的文本。显式 `null`、格式错误等均不生成候选。实现见[采样分支](../../crates/core/src/runtime/prompt_prediction.rs)与[单次传输](../../crates/core/src/llm/transport.rs)。

## 谁拥有在途请求和结果

- Core Server 的[独立 RPC 任务组](../../crates/core-server/src/transport/request_loop.rs)最多接纳 16 个未结束请求，超限返回 busy；[预测协调器](../../crates/core-server/src/application/agent/message_prediction.rs)另以 3 个许可限制采样。等待模型时不占普通 RPC worker。
- 同一会话、目标回复、模型与配置版本、历史版本、偏好代次和执行授权共用一次采样；成功候选和成功的“无候选”均可在进程内复用。缓存最多保留 64 个请求条目，热种子最多 4 份、合计估算不超过 32 MiB。预算不是进程实际内存上限。
- 同一会话的新目标取代旧请求；按到达序号防止较慢的旧查询反过来取消新模型请求。开始新 Turn 或清理被删除会话时按会话失效；撤销执行授权、清理全局上下文或更新预测偏好会清除缓存。关停时停止接纳并取消在途采样。切换到另一聊天本身不取消共享请求；结果返回前仍校验有效性。
- 区分“没有合适建议”和请求错误。模型失败返回无候选并短暂退避，后续重新打开可再试；不会因一次模型错误立即发第二次 HTTP 请求。RPC 本身出错（如回复刚完成但持久化尚不可见）时，仍订阅的 Renderer 最多在 1 秒后重试一次。两者都不能阻断正常输入。

## 用量与恢复

只有 HTTP 发送实际开始才计为一次请求；缓存命中、发送前容量不足或取消不产生新请求记录。调用结束后按请求 ID 写入独立的 `message_prediction_usage`：即使返回空候选、输出被丢弃或发送后取消，也按可得用量登记。Token 未知时不补作实测值；如果发送后进程中断或写入失败，不能凭本地无记录就断定服务商未收费。统计计入请求数、可得 Token 与估算费用，不增加聊天消息数；按范围清空统计会标记对应记录，不是删除聊天。

**候选正文不写入预测账本、消息或草稿。** 只有用户接受后才按普通草稿处理。重启时恢复的是偏好和历史，不是原候选；再次打开符合条件的聊天可能产生一次新采样。计量真源见[存储服务](../../crates/core/src/storage/service/settings.rs)与[用量汇总](../../crates/core/src/storage/usage_repository.rs)。

## 回归入口

```bash
cargo test --locked -p mycopilot-core --lib prompt_prediction
cargo test --locked -p mycopilot-core-server --bin core-server message_prediction
```

[采样测试](../../crates/core/src/runtime/tests/prompt_prediction.rs)检查热路径前缀、冷路径工具输出丢弃、私有回放和 HTTP 错误不重试；[协调器集成测试](../../crates/core-server/src/application/agent/tests/message_prediction.rs)用本地假 Provider 和临时磁盘数据库验证请求合并、重开冷恢复、关闭开关时取消与已发送记账。[存储测试](../../crates/core/src/storage/service/tests/settings.rs)另核对幂等计量、统计清除与零聊天消息贡献。这些测试不证明线上服务商的缓存命中、实际账单或建议质量。
