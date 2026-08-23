---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-23
---

# 设置与配置

## 配置所有权

设置界面只是编辑器，不是权威存储。不同设置有不同所有者：

| 配置                           | 权威位置                  | 说明                                                       |
| ------------------------------ | ------------------------- | ---------------------------------------------------------- |
| 语言、颜色模式、主题           | Renderer `localStorage`   | 启动前即可读取，不包含敏感值                               |
| UI、Prompt、用量显示偏好       | SQLite                    | 通过 Host API 由 Rust Core 读写                            |
| 项目与项目元数据               | SQLite                    | 目录选择由 Main 原生对话框授权                             |
| Provider、Model、Tavily        | SQLite                    | 保存前由 Rust Core 严格校验                                |
| 图片生成 Profile               | SQLite + Credential Store | 配置与凭据分离                                             |
| MCP Server                     | Core Server Registry      | launch authorization 与启用状态独立                        |
| Skill 启用/安装状态            | SQLite + 受管 Skill 目录  | package revision 与来源受约束                              |
| 子 Agent 模板（UI：Subagents） | SQLite                    | 可有全局或项目作用域                                       |
| Automation 任务配置            | SQLite                    | 在 Scheduled 编辑；包含 revision、目标、调度与冻结权限投影 |

对应入口见 [`SettingsPage.tsx`](../../src/renderer/src/features/settings/SettingsPage.tsx)、
[`FrontendConfigProvider.tsx`](../../src/renderer/src/config/FrontendConfigProvider.tsx)、
[`ModelSettingsProvider.tsx`](../../src/renderer/src/config/ModelSettingsProvider.tsx) 和
[`settings.rs`](../../crates/core/src/storage/service/settings.rs)。

## 启动水合

App startup gate 分别等待项目、模型设置及其他权威状态加载。Renderer 可以保存表单草稿和加载状态，
但不得在 Host 请求失败时把默认值回写为“已保存配置”。切换项目或重新连接 Core Server 后，以新的 revision
和 scope 重新水合。

外观与语言由 `FrontendConfigProvider` 从版本化 localStorage 值归一化；无效值回退到代码默认值。
其余持久配置通过 `features/storage/storageClient.ts` 或领域 Host API 访问 Core Server/Rust Core 的 SQLite 服务。

## 保存与并发

- 具备 revision/epoch 的配置使用 compare-and-swap；版本冲突必须重新加载，不能覆盖较新值。
- Provider URL、模型标识、上下文窗口和价格字段由 Rust Core 校验，Renderer 校验只用于即时反馈。
- MCP 的“保存配置”“授权启动”“启用连接”“调用审批”是独立操作；离开未保存表单前必须确认。
- Skill 安装使用 prepare/commit 两阶段并绑定冻结候选，不把 UI 草稿当安装授权。
- 图片生成凭据遵循 keep/replace/clear 意图，不能用空字符串隐式覆盖未知密钥。

### Automation 与当前设置

Automation 不是 Settings 页面中的第二份模型/权限配置。保存任务时，Core Server 根据 permission mode v1
把 `default`、`full` 或 `custom` 解析为完整权限 snapshot；新 Conversation 的 reasoning 只投影自所选
模型配置。后续 Run 使用已冻结权限，而当前 `full`/`custom` enablement 仅作为撤销上限，不能自动扩宽
旧 snapshot。修改自定义权限、模型配置、项目路径或当前电脑 timezone 后，必须理解以下边界：

- 只有有效的 Automation update 才会重建任务配置 snapshot 或解除 `blocked`；
- 关闭 `full`/`custom` 会阻止对应任务后续 admission，重新开启不会自动解除 block；
- 同一 model id 的 Provider 配置变化会影响执行，Automation v1 不持有独立 reasoning override；
- Scheduled UI 保存时用当前电脑 IANA timezone 重建 schedule，当前没有独立时区选择器。

完整契约见 [Scheduled Automation](../subsystems/scheduled-automations.md)。

## 敏感值

模型 Token 与 Tavily Key 当前属于本地模型设置；任何读取接口都必须避免进入日志、Trace、Agent 模型
上下文和错误字符串。图片生成及 MCP 审批密封数据使用独立 Credential Store 边界；后端不可用时按
各领域的 fail-closed/进程内降级规则处理。

禁止：

- 将 Token 放入 MCP argv、环境示例、测试 fixture 或截图；
- 把密码字段的原值回显给 Renderer 以证明“已保存”；
- 在配置错误中拼接完整请求、header、密封 payload 或用户文件内容；
- 把开发环境私有凭据当作签名发行版的持久凭据方案。

## 设置页范围

当前设置导航包含 General、Profile、Appearance、Configuration、Personalization、Usage & Billing、Skills、
Subagents、MCP、Environment 和 Archived Conversations。增加页面时需同时处理：导航与搜索、多语言、
作用域、启动水合、脏表单离开保护、错误恢复、测试与本文件的所有权表。

## 变更检查表

- 配置 DTO 与 parser 是否拒绝未知/无效字段；
- 默认值是在 Renderer、Main、Core Server 还是 Rust Core 定义，是否只有一个权威来源；
- revision/CAS、重复提交和重启后的行为是否有测试；
- reset/backup 是否应保留该配置；删除项目是否应删除该配置；
- Automation 是否需要重建冻结 snapshot、阻断后续 Run 或使现有任务进入 blocked；
- 敏感字段是否避开日志、Trace、IPC event 和 model projection；
- 是否更新相关子系统文档与恢复 Runbook。
