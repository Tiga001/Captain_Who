---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-31
---

# 通用通知

通用通知把普通根任务和 Scheduled Automation 的持久事实投递到系统原生通知，并把点击转换为受限的应用内导航意图。通知不是 Agent 事件流、Automation attention 或业务终态的替代品；各业务页面仍从 Core Server 读取权威状态。进程组合见 [Electron Host](../architecture/electron-host.md)，传输边界见 [IPC 与协议](../architecture/ipc-and-protocol.md)。

## 职责边界

| 层                      | 当前职责                                                                                                               |
| ----------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| Rust Core / Core Server | 在业务提交边界生成不可变事实；去重、合并、替代、过期和解决；持久化批次、lease、投递 disposition、已读状态和设置        |
| Electron Main           | 领取并复核批次；应用前台抑制策略；格式化本地化文案；调用 Electron `Notification`；ACK/release；把点击送到受信 Renderer |
| Preload                 | 只暴露 settings、`markSeen`、event/resync 和 `NotificationOpenRequest`；严格解析 Main 事件                             |
| Renderer                | 在 General Settings 编辑通知设置；接收点击导航；按 event/resync 刷新需要的权威快照                                     |

Renderer 无权 claim、validate、acknowledge、release 或 suppress 批次。Main 不从 Agent 文案猜测完成状态，也不保存第二份通知事实。

## 事实来源与种类

`NOTIFICATION_SCHEMA_VERSION = 1`。事实来源只有：

- `human_root`：用户直接发起的普通根 Conversation；不包括 Scheduled 注入的 HumanRoot turn、子 Agent、dispatcher 或历史快照。
- `automation`：Scheduled task/Run 的终态、重要更新或配置阻塞。

当前 kind 为：

```text
task_completed | task_failed | task_cancelled | approval_required
automation_completed | automation_failed | automation_cancelled
automation_important_update | automation_configuration_blocked
```

普通任务完成、失败、取消和待审批事实与对应消息终态或 pending-action 变更在同一存储服务边界提交。完成/取消默认保留 24 小时，失败/待审批默认保留 7 天；过期、删除、已解决或设置禁用会在 Rust Core 复核时抑制。Automation 的业务判定仍以 [Scheduled Automation](./scheduled-automations.md) 为准。

通知只保存受限 subject：`prompt_excerpt`、`automation_title` 或 `attachment_task`。不得把完整结果、原始错误、命令、审批 payload、附件正文或绝对路径写入通知文案。展示 subject 最多 48 个 grapheme；`showTaskContent=false` 时使用不含任务内容的通用文案。

优先级从低到高是 `completed`、`cancelled`、`important_update`、`failed`、`configuration_blocked`、`approval_required`。合并与点击目标选择使用该顺序，不能按到达时间或 UI 文案重新推断。

## Rust Core-owned 批次与原生投递

```text
business transaction
  -> immutable notification fact
  -> Rust Core-owned dedupe / supersession / batch
  -> Main claim(lease)
  -> Rust Core validate
  -> native show or durable suppression
  -> Rust Core acknowledge or release
```

通用 `notification_events` 保存不可变事实，`notification_batches` 保存原生投递 authority。Automation 的旧投递表和 RPC 只保留为 producer ledger/兼容边界，不能成为第二个 native producer。

Main 的 `SystemNotificationCoordinator` 当前：

- Host 初始化期间保持暂停；首个应用窗口可见后解除暂停并立即 drain，随后每 30 秒轮询；event/resync 只用于提前唤醒。
- 每次最多 claim 10 个批次，lease 60 秒，原生 show timeout 15 秒。
- 显示前再次 validate；平台不支持、应用在前台、事实已删除/解决/过期或设置禁用时持久记录相应 suppression。
- 原生失败后 release，Rust Core 最多尝试 5 次并使用不超过 60 秒的有界退避，耗尽后抑制。
- 同批次 native update 最快每 2.5 秒一次；Electron 39 没有稳定 replace-in-place identity 时，普通计数增长不会制造重复 toast。

只有 Electron 发出 `show` 后才能 ACK。若 ACK 临时失败，同一 Main 进程只重试 ACK；若进程恰在“系统已显示、ACK 尚未提交”的窗口崩溃，lease 恢复后可能再显示一次。当前不存在 OS 级 exactly-once receipt。

## 点击、已读与导航

点击原生卡片时，Main 重新读取该批次仍有效的 items，并只对用户实际看到的 event id 建立 `NotificationOpenRequest`：

```text
application
conversation { conversationId, messageId?, approvalActionId? }
automation { automationId, runId? }
```

Main 随后对这组精确 event id 调用 `markSeen`；不会把后来加入批次但用户未看到的事实一并已读。若批次成员在显示后解决或删除，Main 会撤回已经失真的原生卡片；所属 Conversation、Scheduled 或审批页面仍是权威入口。

Main 只向最近一个发送过 `openRequestedReady`、仍存活且受信的 Renderer 交付点击。没有 ready Renderer 时 FIFO 暂存最多 32 个请求；Renderer crash、销毁或主 frame 导航会撤销 readiness。交付前 Main 恢复、显示并聚焦窗口。Renderer 只按 typed destination 导航，不能接受通知提供任意 URL。

`notification.event` 带单调 `sequence`，`notification.resync` 在 Rust Core 启动后提供 `lastSequence`。它们表示“可能需要刷新”，不是通知已显示、业务已完成或 pending approval 仍有效的证明。

## 设置与当前 UI

设置由 Rust Core 持久化并以 `revision`/CAS 更新：

- `enabled`
- `soundEnabled`
- `showTaskContent`
- `humanCompletedEnabled`
- `humanFailedEnabled`
- `humanApprovalEnabled`
- `humanCancelledEnabled`

General Settings 将普通任务的四个开关组合为：

| 模式      | 四个普通任务开关                                                      |
| --------- | --------------------------------------------------------------------- |
| Never     | 全部关闭，但保持全局 `enabled=true`，因此不会顺带关闭 Automation 通知 |
| All       | 全部开启                                                              |
| Necessary | 完成关闭；失败、审批和取消开启                                        |
| Custom    | 任意其他组合，通过明细抽屉编辑                                        |

旧数据若 `enabled=false`，UI 读取时兼容为 Never，并在下一次成功写入时规范化。声音和任务内容开关与普通任务 preset 分离。

Renderer 当前没有通用通知 inbox、列表或 badge。协议已定义 list/summary，Main 也会为点击快照使用 list，但这些方法没有暴露为 Renderer Host API；不要根据协议类型误写成已交付的通知中心。

Main 的通知语言来自 Renderer 在启动早期同步的窄化 locale mirror，并复用 `src/shared/i18n` 翻译。生产 `NotificationLocaleStore` 只把经过共享 language registry 校验的 language 以 `notification-locale-v1.json` 原子写入 `userData`，使冷启动时也可格式化原生通知；Renderer 的 frontend language 仍是产品设置真源，Main 文件不是第二份完整设置或语言目录。测试可使用易失 mirror。

## 协议与安全不变量

1. Notification schema v1、Automation schema v1/permission v2 与 SQLite canonical schema v35 是不同版本线。
2. 业务事务写事实；event/resync、原生 toast 和 Renderer 文案均不能反向生成事实。
3. claim token、lease、validation 和 disposition 只存在于 Main ↔ Core Server Host-only 边界。
4. 原生文案只使用安全、有限、本地化的 subject；完整业务内容留在所属页面。
5. 点击只导航到共享 parser 接受的 application/conversation/automation destination。
6. 普通根任务、Automation 和子 Agent 的来源语义不能混用；子 Agent 不直接生成 `human_root` 通知。
7. shutdown 的第一个异步等待前必须停止 coordinator，避免 drain 惰性重启 Core Server。

## 代码真源

- TypeScript 协议与 parser：`packages/protocol/src/notifications.ts`
- Rust DTO/method：`crates/protocol-rs/src/notifications.rs`、`crates/protocol-rs/src/methods.rs`
- Core Server 应用服务：`crates/core-server/src/application/notification.rs`
- 普通根任务生产者：`crates/core-server/src/application/agent/human_root_notifications.rs`
- Repository/service：`crates/core/src/storage/notification_repository.rs`、`storage/service/notifications.rs`
- Main coordinator：`src/main/notifications/systemNotificationCoordinator.ts`
- 文案与语言：`src/main/notifications/notificationPresentation.ts`、`notificationLocaleStore.ts`、`src/shared/i18n/`
- Main/Preload：`src/main/ipc/notificationIpc.ts`、`src/preload/NotificationIpcBridge.ts`
- Renderer settings：`src/renderer/src/features/notifications/`、`features/settings/pages/GeneralSettingsPage.tsx`

## 测试与变更检查表

关键测试包括：

- `packages/protocol/src/notifications.test.ts`
- `crates/core/src/storage/service/tests/notifications.rs`
- `crates/core-server/src/application/agent/tests/human_root_notifications.rs`
- `src/main/core/systemNotificationCoordinator.test.ts`
- `src/main/core/ipc.notifications.test.ts`
- `src/main/core/notificationPresentation.test.ts`
- `src/preload/NotificationIpcBridge.test.ts`
- `src/renderer/src/features/notifications/__tests__/`

- [ ] 新 kind 同步来源、优先级、TTL、dedupe/supersession、presentation 和导航目标。
- [ ] 事实与业务终态在同一持久边界提交，并覆盖删除、解决、过期和 startup reconciliation。
- [ ] Host-only RPC 未进入 Renderer allowlist；事件和点击都经过严格 parser。
- [ ] unsupported、foreground、show/failed/timeout、lease recovery、ACK retry 和进程崩溃窗口有测试。
- [ ] 设置更新使用 CAS；Never 不会意外关闭 Automation 通知。
- [ ] 文案、subject 和日志不包含结果正文、secret、原始错误或路径。

## 当前限制

- 当前只提供原生通知、设置与点击导航，没有应用内通知中心或 badge。
- Electron 39 不提供可靠的稳定替换 id；合并卡片更新采用安全降级。
- 系统显示与 Rust Core ACK 之间的崩溃窗口可能导致一次重复通知。
- 点击队列最多 32 项，且产品仍按单个主要 Renderer 设计。
