---
status: current
audience: developers
owner: engineering
last_verified: 2026-09-03
---

# 设置与配置

## 配置所有权

设置界面只是编辑器，不是权威存储。不同设置有不同所有者：

| 配置                           | 权威位置                  | 说明                                                              |
| ------------------------------ | ------------------------- | ----------------------------------------------------------------- |
| 语言、颜色模式、主题           | Renderer `localStorage`   | 启动前即可读取；语言另有 Main 私有通知 locale 镜像                |
| UI、Prompt、用量显示偏好       | SQLite                    | 通过 Host API 由 Rust Core 读写                                   |
| 系统通知设置                   | SQLite                    | Rust Core 保存 schema v1 + revision；General 页面以 CAS 更新      |
| 项目与项目元数据               | SQLite                    | 目录选择由 Main 原生对话框授权                                    |
| Provider、Model、Tavily        | SQLite + Credential Store | SQLite 保存配置/reference；secret 由凭据 backend 保存             |
| 图片生成 Profile               | SQLite + Credential Store | 配置与凭据分离                                                    |
| MCP Server                     | Core Server Registry      | launch authorization 与启用状态独立                               |
| Skill 启用/安装状态            | SQLite + 受管 Skill 目录  | package revision 与来源受约束                                     |
| 子 Agent 模板（UI：Subagents） | SQLite                    | 工作区级模板库，通过 binding 分配给一个或多个项目                 |
| Browser 偏好与下载设置         | SQLite + Electron Main    | Rust Core 保存 revision/实际目录；Main 持有原生选择与文件操作能力 |
| Automation 任务配置            | SQLite                    | 在 Scheduled 编辑；包含 revision、目标、调度与冻结权限投影        |

对应入口见 [`SettingsPage.tsx`](../../src/renderer/src/features/settings/SettingsPage.tsx)、
[`FrontendConfigProvider.tsx`](../../src/renderer/src/config/FrontendConfigProvider.tsx)、
[`ModelSettingsProvider.tsx`](../../src/renderer/src/config/ModelSettingsProvider.tsx) 和
[`settings.rs`](../../crates/core/src/storage/service/settings.rs)。

## 启动水合

App startup gate 分别等待项目、模型设置及其他权威状态加载。Renderer 可以保存表单草稿和加载状态，
但不得在 Host 请求失败时把默认值回写为“已保存配置”。切换项目或重新连接 Core Server 后，以新的 revision
和 scope 重新水合。

外观与语言由 `FrontendConfigProvider` 从版本化 localStorage 值归一化；无效值回退到代码默认值。当前 Catalog
包含 `zh-CN`、`zh-TW`、`en-US`、`en-GB`、`ko-KR`、`ja-JP`、`fr-FR`、`it-IT` 与 `ru-RU`。Renderer
仍是语言 authority；Main 只把已验证的语言枚举原子写入权限受限的 `notification-locale-v1.json`，供冷启动时格式化原生通知，不能在该文件扩展第二套配置。
其余持久配置通过 `features/storage/storageClient.ts` 或领域 Host API 访问 Core Server/Rust Core 的 SQLite 服务。

新安装现在以空 Model Catalog 启动，不再预置模型 ID、价格或连接。首次成功读取可以得到 `models: []`；用户必须显式保存至少一个有效 Provider/Model 后才能开始依赖模型的任务。Renderer 不得把旧的展示默认值回写成隐式 Catalog。

## 保存与并发

- 具备 revision/epoch 的配置使用 compare-and-swap；版本冲突必须重新加载，不能覆盖较新值。
- Provider URL、模型标识、上下文窗口和价格字段由 Rust Core 校验，Renderer 校验只用于即时反馈。
- MCP 的“保存配置”“授权启动”“启用连接”“调用审批”是独立操作；离开未保存表单前必须确认。
- Skill 安装使用 prepare/commit 两阶段并绑定冻结候选，不把 UI 草稿当安装授权。
- 模型、Tavily 与图片生成凭据统一遵循 `keep | replace | clear` 意图，不能用空字符串隐式覆盖未知密钥。
- General 的 Custom 权限除 read/write/command/FileChange 外，还包含 `builtinExecution` 审批开关；它控制符合条件的应用内置 capability/Tool 与 BrowserRisk 是否需要本次人工点击，但不会自行激活 capability，也不跳过 target、risk、来源、policy、audit 或执行前复核。
- Agent 模板名称与 machine key 在工作区级模板库内唯一；启用状态和内容使用 revision/CAS，项目 assignment 独立且每个项目最多 32 个模板。未分配、已停用或模型不可用的模板不能用于 spawn。
- Browser 的链接打开目标使用 `system | builtin`；下载目录与“每次询问”只影响用户手动下载，Agent 下载不会停在原生保存对话框中。Renderer 只接收安全 display path，不取得 Host 保存的真实 custom directory authority。

### 人机交互设置

“允许智能体向人类提问”默认开启，由 Host 独立保存 enabled/revision，使用 `host.humanInteraction` 读取、CAS 更新和接收变更通知。它不属于 Prompt Preferences，个性化整页保存不能覆盖此项。关闭只阻止新提问，已有问题仍能回答或忽略。第 1 轮完成存储和接口；个性化中的“人机交互”开关页面在第 4 轮接入，当前未完成执行链路的工具不会对模型暴露。完整设计见[向用户提问](../subsystems/human-interaction.md)。

### 系统通知设置

Rust Core 的 `notification_settings` 保存全局启用、声音、是否显示安全任务摘要，以及普通根任务完成/失败/需批准/取消四类开关。General 页面提供“从不、全部、仅必要、自定义”预设；这些预设只组合普通任务字段，不能被理解为逐项控制 Automation 通知。更新必须带 `expectedRevision`，冲突后重新加载再保存。

原生通知只在应用不处于前台时尝试显示。关闭任务内容预览只影响 Main 的通知 presentation，不删除 Conversation、Automation 或 notification event 中的权威事实。语言变化会异步同步 Main locale 镜像；镜像写入失败不得回滚 Renderer 当前语言。

### Automation 与当前设置

Automation 不是 Settings 页面中的第二份模型/权限配置。保存任务时，Core Server 根据 permission mode v2
把 `default`、`full` 或 `custom` 解析为完整权限 snapshot；新 Conversation 的 reasoning 只投影自所选
模型配置。后续 Run 使用已冻结权限，而当前 `full`/`custom` enablement 仅作为撤销上限，不能自动扩宽
旧 snapshot。修改自定义权限、模型配置、项目路径或当前电脑 timezone 后，必须理解以下边界：

- 只有有效的 Automation update 才会重建任务配置 snapshot 或解除 `blocked`；
- 关闭 `full`/`custom` 会阻止对应任务后续 admission，重新开启不会自动解除 block；
- 同一 model id 的 Provider 配置变化会影响执行，Automation v1 不持有独立 reasoning override；
- Scheduled UI 保存时用当前电脑 IANA timezone 重建 schedule，当前没有独立时区选择器。

完整契约见 [Scheduled Automation](../subsystems/scheduled-automations.md)。

## 敏感值

模型 Token、Tavily Key 与图片生成 API Key 使用同一安全边界：SQLite 只保存不透明 credential reference、
状态和非秘密配置；secret 由 Credential Store 保存。具备稳定签名身份的发行构建使用操作系统凭据
存储，未签名 macOS 开发构建使用应用数据根内的私有文件 backend（目录 `0700`、文件 `0600`）。
Renderer 只接收 `missing | configured | unavailable` 状态和用户本次新输入的替换值，既不能读回已有
secret，也不能取得 reference。任何读取接口都必须避免让密钥进入日志、Trace、Agent 模型上下文和错误字符串。

共享 `CredentialInput` 只有三种常规视觉形态：未配置时是空白写入框与可见性开关；已配置时显示固定
掩码、配置状态、替换与清除图标；替换中显示新的空白输入、可见性开关与行内取消。`clear` 是 Host 协议
中的显式 mutation，经现有确认弹窗提交，不是占用额外行高的第四种表单状态。模型、搜索与图片生成均
使用该组件，图片生成配置同样支持清除已保存 Key。

Credential Store 暂时不可用时，公开配置仍应可加载并显示 `unavailable`；只有需要该连接的运行 fail
closed。数据库事务和 SQLite mutex 内不得执行操作系统凭据或开发凭据文件 I/O。

禁止：

- 将 Token 放入 MCP argv、环境示例、测试 fixture 或截图；
- 把密码字段的原值回显给 Renderer 以证明“已保存”；
- 在配置错误中拼接完整请求、header、密封 payload 或用户文件内容；
- 把开发环境私有凭据当作签名发行版的持久凭据方案。

## 设置页范围

当前设置导航包含 General、Profile、Appearance、Configuration、Personalization、Usage & Billing、Skills、
MCP、Browser、Subagents、Environment 和 Archived Conversations。Browser 页面还拥有 Settings/History/Download History 子视图。增加页面时需同时处理：导航与搜索、多语言、
作用域、启动水合、脏表单离开保护、错误恢复、测试与本文件的所有权表。

## 变更检查表

- 配置 DTO 与 parser 是否拒绝未知/无效字段；
- 默认值是在 Renderer、Main、Core Server 还是 Rust Core 定义，是否只有一个权威来源；
- revision/CAS、重复提交和重启后的行为是否有测试；
- reset/backup 是否应保留该配置；当前 reset 从 exact current v36 或 exact v35 保留 allowlisted 配置与 credential reference，v36 还保留人机交互设置及 revision，不复制或恢复操作系统 secret。通知事件、Browser history/download records 或 Agent template library 不保留；
- 删除项目是否应删除该配置或仅移除 Agent template assignment；
- Automation 是否需要重建冻结 snapshot、阻断后续 Run 或使现有任务进入 blocked；
- 敏感字段是否避开日志、Trace、IPC event 和 model projection；
- 是否更新相关子系统文档与恢复 Runbook。
