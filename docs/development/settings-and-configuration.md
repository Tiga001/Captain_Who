---
status: current
audience: developers
owner: engineering
last_verified: 2026-09-09
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

### 模型快捷选择

Composer 的 `/` 菜单首项为 `model`，与 `capabilities` 一样是二级导航命令，不包含普通命令的 `execute` 回调。进入模型面板只改变菜单视图，不提交消息。面板不提供筛选输入，返回按钮、Escape 或无修饰键的 Backspace/Delete 恢复原命令列表，并保留原 `/` 查询。

模型列表复用 `ModelSettingsProvider.enabledModels`，与 Composer 右下角模型入口使用同一份配置。展示内容从模型配置投影出实际模型 ID、厂商、上下文容量和输入模态；`contextWindowTokens` 缺失时使用既有运行默认值 `DEFAULT_MODEL_CONTEXT_WINDOW_TOKENS = 128_000`，显示为 128K，非法容量显示“上下文未知”，不根据模型名称猜测。两个入口使用同一模型选择处理函数，只修改 Composer 草稿。运行中允许选择下一轮模型与权限，真实 Provider 切换及手动压缩期间仍禁用选择；发送下一轮前沿用 Host 的空闲检查、Provider 切换确认和压缩流程。

当前 Run、审批续跑与 Guidance 保持原配置；队列按每项的 `modelId` / `permissionMode` 发送，不回写覆盖用户后来选择的草稿。异步提交与 Provider 完成通知只能更新仍属于原提交的草稿字段。上下文用量在 Run 活跃时采用 Host 事件快照，停止草稿预估 RPC 并作废此前在途预估；结束后才为当前草稿刷新预估。

### 轻量上下文模式

个性化页面的工作模式、语气等选择及“轻量模式”切换后立即保存；“保存”按钮只提交自定义指令，其他设置的自动保存不能提交尚未保存的指令草稿。“轻量模式”位于“个性”选择框下方，说明为“精简基础上下文和工具。”；标题后的小问号复用模型配置的帮助入口和弹窗样式，弹窗标题为“轻量模式”，正文为“打开轻量模式后，基础系统提示词、基础工具集合和对应工具说明会被精简。在新一轮次对话生效。”，确认按钮为“知道了”。开关默认关闭，对应全局 `AgentPromptPreferences.contextProfile = full | minimal`，不把 `coding | general` 改成第三种工作模式，也不改写图片生成、搜索、人机交互、协作、浏览器、Skill 或 MCP 的原配置。模式切换从新轮次生效，关闭后恢复完整基础提示词和工具。

输入框 `/` 菜单的“能力中心”在首行提供同一轻量模式开关。快捷入口保存前读取最新偏好，只修改 `contextProfile`；两个入口通过偏好变更通知同步，保留其他个性化设置及未保存的自定义指令草稿。

新根 Run 在接纳事务中冻结模式；该 Run 委派的 Wake 继承此值，审批、提问及进程恢复沿用原 Run 策略。保存不改变正在执行的任务树，也不启动模型请求。`agent.promptPreferencesChanged` 通知只携带 `contextProfile` 与 `updatedAt`，不传播自定义指令；Host 使上下文缓存失效，Renderer 为空闲会话刷新预览，活跃 Run 继续采用本轮快照，结束后刷新下一轮预览。提示词、工具与计量的对应关系见[Agent Runtime](../architecture/agent-runtime-and-providers.md#轻量上下文模式)。

### 人机交互设置

“允许智能体向人类发起提问与协作”默认开启，由 Host 独立保存 enabled/revision，使用 `host.humanInteraction` 读取、CAS 更新和接收变更通知。它不属于 Prompt Preferences，个性化偏好保存不能覆盖此项。关闭只阻止新提问，已有问题仍能回答或忽略。完整设计见[向用户提问](../subsystems/human-interaction.md)。

### 能力中心

Composer 的 `/` 菜单提供“能力中心”二级面板。软件能力区最上方是轻量模式，其后为图片生成、联网搜索、人机交互、多智能体、浏览器自动化；MCP 区按外部服务器稳定 ID 列出开关。该入口不管理 Skill，不建立第二份 capability settings 或新的授权来源。保存期间仍拦截同一开关的重复操作，但保持行高亮和透明度；仅加载或不可用状态淡化整行。Backspace/Delete 在菜单或空搜索框中等价于返回按钮，搜索框非空时正常删字；输入法组合期间及错误弹窗内不触发返回。

| 菜单项       | 既有权威与写入路径                                                               | 生命周期保持不变                            |
| ------------ | -------------------------------------------------------------------------------- | ------------------------------------------- |
| 图片生成     | ImageGeneration configuration 的 enabled/revision；与图片 Skill 开关共用 profile | 原有发现、加载与执行校验                    |
| 联网搜索     | ModelSettings 的 searchMode/configurationRevision；凭据始终 keep                 | 原有请求与执行时校验                        |
| 人机交互     | 独立 HumanInteraction settings revision/CAS                                      | 请求快照及新批次接纳时校验；已有问题可结算  |
| 多智能体     | AgentCollaboration settings revision/CAS                                         | 保持既有 Run/任务继承的冻结策略             |
| 浏览器自动化 | 内置 MCP capability policyRevision                                               | 保持原有禁用撤销与停止行为                  |
| 外部 MCP     | MCP server registry/config epoch 前置条件                                        | 保持启动授权、enable/disable 与连接生命周期 |

菜单逐行防重复提交，等待权威结果再更新状态；不显示“正在保存”文字，不改变行高。配置不足、授权缺失和保存失败复用 `ConfirmationDialog`，底部只显示“知道了”。菜单不代办 launch authorization，不把“用户希望开启”显示成“后端已开启”；不确定结果需重新查询，不盲目重试或回滚。

Host 新增零 payload 的 `storage.onModelSettingsChanged`、`imageGeneration.onChanged`、`mcp.onBuiltinCapabilitiesChanged` 失效通知。它们覆盖对应写入及不确定失败，图片 Skill 别名也触发图片配置失效；Core Server 启动/重连触发重新查询。外部 MCP、人机交互和多智能体继续使用原有领域事件。通知不携带凭据，也不冒充已保存结果。Renderer 查询有序列/写入围栏，旧响应不能覆盖后来的权威状态；窗口焦点恢复时补查。

联网快捷开关通过 `ModelSettingsProvider.saveSearchMode` 串行保存，在真正执行写入时基于上一份权威配置构造 searchMode 单项变化并保留其他字段。排队的模型/URL 保存也保留最新 searchMode；只有显式清除 Tavily 凭据时会同时关闭搜索。通知刷新等待保存队列，避免覆盖正在提交的设置。图片设置后台刷新保留未保存的配置和凭据草稿，以及对应的旧 CAS 基线；若其他窗口修改了配置，后续保存仍由后端冲突检查裁决。冲突或结果不确定时，重新读取权威状态，保留已编辑字段和凭据意图、刷新未编辑字段，不自动重试；用户核对后再次保存才使用新的 revision。

本次不修改 Harness 能力生命周期、检查点或 canonical schema，无需重置开发聊天数据。

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
- reset/backup 是否应保留该配置；当前 reset 保留 allowlisted 配置与 credential reference（含 v44 的上下文模式），受支持旧版来源与默认值见[存储生命周期](../architecture/storage-and-data-lifecycle.md#schema-发布策略)，不复制或恢复操作系统 secret。通知事件、Browser history/download records 或 Agent template library 不保留；
- 删除项目是否应删除该配置或仅移除 Agent template assignment；
- Automation 是否需要重建冻结 snapshot、阻断后续 Run 或使现有任务进入 blocked；
- 敏感字段是否避开日志、Trace、IPC event 和 model projection；
- 是否更新相关子系统文档与恢复 Runbook。
