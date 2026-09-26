---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-09-26
---

# 手动上下文压缩与快捷命令

用户在空白输入框中键入 `/` 打开本地命令面板。面板提供七项命令：压缩上下文、新聊天、创建聊天分支、使用情况和计费、置顶/取消置顶、重命名、归档。新聊天、元信息操作和用量设置复用现有应用入口。

## 输入与展示

面板按中文名称或稳定英文别名 `compact`、`new`、`fork`、`usage`、`pin`、`rename`、`archive` 实时筛选，突出匹配文字。鼠标悬停与方向键共享选中行，Enter 执行该行；Escape 收起面板并保留文字，Tab 按正常焦点顺序导航。已有附件不妨碍通过空白文本打开面板。

只有真实键入 ASCII `/` 才开启命令会话，正文中的斜杠、粘贴路径和恢复草稿不会开启。IME composition、229 键码及确认后的短暂 Enter 保护沿用 Composer。已知禁用命令不会进入发送或排队路径，表单和发送按钮与 Enter 使用同一分流。未知 Slash 文本可以作为普通消息发送。执行命令仅清理查询文本，保留附件、Skills、权限、模型、项目和排队消息。

手动压缩复用 Provider transition 的时间线分割线样式与运行文字效果，开始显示“正在压缩上下文”，成功显示“上下文历史已压缩”。成功分割线右侧提供 Split 分支按钮，从该次手动压缩的精确操作边界创建分支；运行、失败、中断、取消和 noop 状态不提供此按钮。界面不显示压缩取消按钮，后端取消接口仍保留。时间线锚点只用于展示，不是计费 owner 或模型游标；原始消息不变。操作按 conversation/operation identity 合并，迟到的 running 通知不能覆盖终态；切换聊天和重新加载从 Host 查询持久状态。完成后刷新上下文占用。

## Host 与 Core Server

公开契约在 `packages/protocol/src/manualContextCompaction.ts`，经过 Host API、固定 Preload 通道、Main 验证后进入 Core Server：

- `agent.startManualContextCompaction`：`conversationId` 与 `requestId`。
- `agent.getManualContextCompactionStatus`：`conversationId`，可选 `operationId`。
- `agent.cancelManualContextCompaction`：`conversationId` 与 `operationId`。
- `agent.manualContextCompaction`：安全操作快照通知。查询是恢复依据。

操作状态包括 running、completed、noop、cancelled、failed、interrupted；阶段为 preparing、generating、committing。终态保留最后阶段。公开 `isBusy` 同时反映取消后的响应收尾期，前端在收到最终释放状态前继续禁止冲突操作。相同 request identity 重放返回原操作，不再次请求模型。

服务在 conversation admission 下接纳已保存、未归档、空闲根聊天，拒绝运行、审批、模型转换、后台命令和冲突维护操作。普通发送、rewrite、Wake/Automation admission、模型转换、fork 和删除使用同一忙碌保护。Renderer 不提交模型 Profile、摘要、cursor 或内部 receipt。

摘要生成复用跨厂商无工具压缩器；从持久日志选择最新完整安全前缀，检查游标前进、工具组闭合和受保护内容。没有新安全内容或没有值得压缩的历史时返回 noop，不发模型请求。空闲压缩可以包含最近完成的一轮，原始聊天时间线和 Exact Archive 不因此删减。

压缩摘要始终使用显式有限输出预算（上限 30,000，再按任务上限和可用上下文空间收紧），不随普通聊天请求省略 HTTP 输出上限而变为无限预算。详见[上下文预留](../architecture/context-management.md#请求输出上限与上下文预留)。

生成在事务外执行。收到的实际用量先独立持久化，然后在提交事务中复核旧 head、历史和模型配置，原子应用摘要、receipt 与操作终态。取消先请求 token 停止；提交成功后取消以 completed 事实为准。失败、提交前取消或过期状态都不替换旧上下文。启动恢复将残留 running 操作置为 interrupted，不自动重放付费请求。

## 用量、存储和分支

操作表和独立用量表属于当前 SQLite canonical schema；当前版本、允许的 exact 升级路径及 reset 限制统一见[存储生命周期](../architecture/storage-and-data-lifecycle.md#schema-发布策略)。`/fork` 和成功分割线的分支入口复用已有持久数据，不另建模型 Run 或重复计费。

用量以 operation 为 owner，冻结请求模型价格，不覆盖上一条助手回复、不增加聊天消息数。失败或取消后已知的实际用量仍计入；未知 token 数量保持未知。清理用量保留幂等凭证，删除聊天前汇入日汇总。请求已在远端处理但本地尚未收到响应时崩溃，无法从本地准确补出厂商账单。

`/fork` 调用 `{ kind: 'latest' }`，由 Host 在执行时解析当前有效模型、最新摘要和最新安全完整历史边界；Renderer 不把最后一条可见消息当作权威边界。没有完整回复时命令置灰，并显示“暂无完整回复可供分支”。

成功分割线的 Split 按钮绑定该条手动压缩的 operation identity，Host 验证操作、已提交摘要和来源后，从其精确边界创建分支。它不会随着后续聊天推进而改为 latest，也不等同于从锚点助手回复创建历史分支。两种入口仅在已保存、未归档的根聊天空闲时可用；Host 仍复核活跃 Turn、审批、后台命令和上下文操作等冲突，不能只依赖前端置灰。

分支复制对应边界内的上下文和展示来源，不复制费用；历史回复分支保持自己的历史边界，不取得其后的手动摘要。分支创建完成后，新旧聊天各自继续，不持续同步。

## 验证入口

- `src/renderer/src/app/__tests__/ChatComposerCommands.browser.test.tsx`：中文筛选、hover/键盘、输入法、路径粘贴、恢复草稿、禁用命令和重复执行。
- `src/renderer/src/app/__tests__/useManualContextCompaction.browser.test.tsx`：持久状态恢复、迟到通知、取消结果与聊天切换。
- `packages/protocol/src/manualContextCompaction.test.ts` 与 `src/main/core/coreServer.manualCompaction.test.ts`：身份和安全字段边界。
- `crates/core-server/src/application/agent/tests/manual_context_compaction.rs`：独立请求、取消/提交竞态、费用、失败和“压缩 → latest fork → 下一轮发送”。
- `crates/core/src/storage/manual_context_compaction_repository/tests.rs`、`crates/core/src/storage/migrations.rs` 与 fork/storage 测试：幂等、重启、新库与旧库拒绝、历史可见性、清理与删除汇总。

所有模型链路测试使用本地模拟服务，存储测试使用临时数据库，不访问实际用户数据或付费模型。
