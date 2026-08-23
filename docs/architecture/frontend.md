---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-23
---

# 前端架构

本文描述 Electron Renderer 当前的 React 组合、依赖方向、状态归属和异步一致性规则。进程边界见 [Electron Host 与进程架构](./electron-host.md)，Host 调用规则见 [IPC 与协议](./ipc-and-protocol.md)。

## 职责边界

Renderer 负责：

- 展示会话、Agent Run、Automation Run、审批、设置和右侧栏 Tool。
- 管理仅影响展示和交互的临时状态。
- 将 Electron Main service 或 Rust Core 的权威快照投影为 UI，并拒绝身份不匹配或过期的异步结果。
- 通过窄化的 Host API 发起持久化、原生能力或后端操作。

Renderer 不负责：

- 直接读写文件、运行命令、持有凭据或访问 Electron/Node API。
- 推断后端持久终态、伪造运行/会话/Artifact 身份。
- 把路径字符串、网页内容或前端开关当成权限授予。

## 目录与依赖方向

当前主要层次为：

```text
main.tsx
  └── App.tsx                 Provider 组合
      └── app/AppShell.tsx    应用级编排
          ├── features/*      领域状态、视图和客户端
          ├── components/*    跨领域展示组件
          ├── config/*        主题、语言、模型和项目 Provider
          ├── host/*          Host API 客户端入口
          └── @mycopilot/protocol / @mycopilot/host-api
```

ESLint 当前强制两条边界：

1. `features/*` 不得依赖 `app/*`。共享逻辑应留在 feature 或共享模块中，由 AppShell 注入回调和上下文。
2. `components/*` 不得依赖 `app/*` 或 `features/*`。共享组件保持领域无关。

Feature 之间可以在明确的产品组合点复用，例如 Files 复用 Git Review 的语言识别与高亮基础设施，右侧栏负责组合 Browser、Terminal、Files、Git Review 和 Agent Center。增加这种依赖时，应避免形成反向依赖或把 AppShell 状态藏入 feature。

## Provider 与启动组合

`App.tsx` 的 Provider 顺序是：

1. `FrontendConfigProvider`
2. `ToastProvider`
3. `ImagePreviewProvider`
4. `AppStartupProvider`
5. `ModelSettingsProvider`
6. `ProjectSettingsProvider`
7. `AppStartupGate`
8. `AppShell`

顺序具有语义：内层 Provider 可以上报阻塞启动阶段，Toast/ImagePreview 可被整个工作区使用，FrontendConfig 在启动遮罩出现前就提供主题和语言。

启动门要求 `core`、`modelSettings`、`projects`、`uiPreferences`、`composerDrafts`、`conversationMetas` 全部 ready。工作区组件会立即挂载，但 ready 前处于 `inert`/`aria-hidden` 状态。这允许各领域启动 effect 执行，同时防止用户操作部分加载的数据。

重试会递增 startup attempt。任何异步加载都应捕获本次 attempt、request sequence 或业务 identity；迟到的旧尝试不得覆盖新状态。

## AppShell 编排边界

`app/AppShell.tsx` 是应用级 orchestration layer，而不是可复用 feature。它当前组合：

- 会话元数据、按需详情加载、活动会话和滚动位置。
- Composer 草稿、附件、模型与 Skill 选择。
- Agent Run 绑定、事件缓冲、流式文本刷新、停止与权威终态对账。
- 待审批动作、命令会话恢复、steer/排队消息、编辑重写和 Provider transition。
- Multi-Agent collaboration store、审批和 observer surface。
- 左侧栏、中央会话页、Automation 的 Scheduled 主视图、右侧栏 workspace/capability 和全屏设置页。

复杂流程应提取为 `app/use*.ts` 或相应 feature hook；AppShell 只保留跨领域组合与顶层回调。Feature 不得反向导入 AppShell。

## UI 组成

主工作区由三列组成：

- 左侧栏：项目、会话和应用入口。
- 中央区域：新会话或交互式 `ConversationSurface`。
- 右侧栏：模块注册表驱动的多页面平台，详见 [右侧栏平台](../subsystems/right-sidebar.md)。

Automation 在 UI 中显示为 `Scheduled`。它由左侧栏入口切换为独立 `primaryView`，覆盖中央会话页和右侧栏，而不是注册成右侧栏页面；左侧栏仍保留，用于切回 Conversation。`AppShell` 只持有视图切换、外部导航请求和本次应用会话的 drawer 宽度偏好，Automation task、Automation Run 和 attention 的业务状态仍来自 Core Server。

设置页以覆盖主工作区的全屏视图呈现。`AppShellWorkspace` 在设置打开时保持挂载，但设为 `inert` 和 `aria-hidden`，从而保留会话、终端和浏览器状态，同时隔离焦点和辅助技术树。

会话视图分两种能力面：

- interactive surface 可以提交、停止、编辑、审批和修改持久 UI 状态。
- observer surface 只能读取精确 root/child 身份下的投影，不获得草稿、提交、审批或持久化写能力。

两者复用展示组件，但 mutation 能力必须通过 props/types 明确区分。Multi-Agent 详细约束见 [Multi-Agent 架构](./multi-agent.md)。

## 状态归属

| 状态                                             | 当前真源                                                             | Renderer 责任                                                   |
| ------------------------------------------------ | -------------------------------------------------------------------- | --------------------------------------------------------------- |
| 语言、明暗偏好、主题 id                          | `localStorage` 的 frontend config                                    | 启动前容错读取；规范化；同步 DOM 属性和 Electron native theme   |
| 模型设置、项目、会话、消息、草稿、UI preferences | Rust Core 管理的 SQLite，经 Storage Host API                         | hydration、局部编辑、按规定顺序写回；不能绕过协议直接访问数据库 |
| 图片生成、MCP、Skill、Agent 模板等管理状态       | Rust Core 或 Electron Main service 的权威快照与 revision             | 使用 CAS/precondition；冲突后刷新，不在前端合并猜测             |
| 图片生成凭据                                     | Rust Core 选择的凭据存储                                             | 只暂存用户当前输入，成功后立即清空；从不回读明文                |
| 活动 Run、pending action、command session        | Rust Core 事件和存储记录                                             | 绑定权威 id、缓冲早到事件、reload 时重新 hydrate/reconcile      |
| Automation task、Automation Run、attention       | Core Server/Rust Core 管理的 SQLite Automation task/Run/event/outbox | 按 `revision`、事件 `sequence` 和请求身份合并；通知只触发刷新   |
| 右侧栏页面、选中项、滚动/局部预览状态            | Renderer 内存                                                        | 按 module 策略保活或卸载；应用重启后重建                        |
| Toast、对话框、焦点、临时请求状态                | Renderer 内存                                                        | 生命周期结束时清理；不得成为业务授权依据                        |

新增状态前必须先确定真源。需要跨重启、跨窗口或参与授权判断的状态不应只存在 React state/localStorage。

## Agent 运行与异步一致性

`useAgentRunLifecycle.ts` 是普通会话运行的主要协调器。关键规则包括：

- submit/rewrite 请求携带会话、消息、附件、模型、权限和 Skill revision 等精确输入。
- `runId` 返回前到达的事件进入有界缓冲；绑定建立后再按顺序归并。
- command session 事件按其持久 owner 路由，不简单依赖当前活动 Run。
- 流式 delta 先短暂批处理，再更新 React 状态和持久化投影，避免每 token 重渲染。
- stop 只是请求；最终 completed/failed/cancelled 状态必须来自后端事件或权威存储对账。
- reload 后重新加载 pending actions、command sessions 和未完成 Run，而不是根据 UI 文本推断。
- edit/rewrite、Provider transition 和 Skill 恢复均使用 epoch/revision，迟到结果不得回退更新后的选择。

后端上下文算法见 [上下文管理](./context-management.md)；Tool 结果投影规则见 [Tool 结果消费者矩阵](../subsystems/tool-result-consumer-matrix.md) 和 [Tool 结果限制](../subsystems/tool-result-limits.md)。

## Automation（UI：Scheduled）

`features/automations/` 是 Scheduled 页面及其 Renderer client、cache、realtime 和表单状态的领域边界；
后端状态机见 [Scheduled Automation](../subsystems/scheduled-automations.md)。主要状态流为：

```text
ScheduledPage
  -> automationClient -> Host API v1 snapshot/mutation
  -> shared task cache keyed by automationId + revision
  <- automation.event(sequence) / automation.resync(lastSequence)
  -> reload authoritative task/run/attention snapshots
```

当前状态所有权和一致性规则如下：

- `useAutomations`、`useAutomationDetail`、`useAutomationRuns` 和 `useAutomationAttention` 可以共享缓存，但缓存不是权威事实。列表、详情、Automation Run 历史和 attention 最终以 Host API 返回的快照为准。
- create 使用可重试的 `requestId`；update、enable/pause 和 delete 使用权威 `revision`/CAS。迟到的旧列表、详情或 mutation 响应不得覆盖更高 revision，已确认删除的 task 不得被在途请求复活。
- `automation.event` 带全局单调 `sequence`、task/run identity 和可选 `resourceRevision`。Renderer 丢弃重复或倒序事件，并把事件视为失效通知；`automation.resync` 即使没有增量 payload 也要求重新加载权威状态。
- 原生通知点击产生的 `AutomationOpenRequest` 只表达导航意图。AppShell 按 request key 处理 Automation task 或精确 Conversation/message 导航；Conversation 尚未水合时先从 Storage Host API 加载，不能用通知文案重建消息。
- 表单只提交 permission mode 和 `permissionModeVersion`。Renderer 的可用性开关与风险确认是交互门，Core Server 仍会解析模式、冻结有效权限，并在每次 Automation Run admission 时复核撤销上限。
- Existing Conversation picker 排除已归档或正在乐观归档的项；Core Server 仍负责确认目标是可写的活跃根 Conversation，并在目标、Project 或 Model 失效时阻止执行。
- 新建和实际保存编辑时，client 在请求边界写入当前电脑识别出的 IANA timezone。只查看 task 时保留服务端已保存 timezone；在另一时区的电脑上保存编辑会按该电脑 timezone 重新规范化，这是当前明确语义。

Scheduled drawer 的展开、最大化、dirty guard、焦点恢复和宽度都是 Renderer 交互状态。布局以 Scheduled 容器自身宽度而非 viewport 为准；当前默认宽度 440 px、可调整范围 380–640 px、列表至少保留 360 px，容器小于 760 px 时 drawer 覆盖列表。宽度偏好只保存在当前 AppShell 会话，应用重启后恢复默认值。

Automation 共享 DTO 使用 `AUTOMATION_SCHEMA_VERSION = 1`，permission mode 使用独立的 v1；它们不是 SQLite canonical schema。当前 SQLite schema 是 v18，Renderer 不读取或协商该数据库版本。

## 设置架构

设置页的页面 id 和导航清单以 `features/settings/SettingsPage.tsx` 为准。当前精确 id 为 `general`、`profile`、`appearance`、`configuration`、`personalization`、`usageBilling`、`skills`、`agentTemplates`、`mcp`、`environment` 和 `archivedConversations`。

设置不是单一存储域：外观中的语言/主题在 localStorage，其他 UI preferences 多数通过 Storage API，MCP/Skill/图片生成使用各自的权威服务。页面组件应调用所属领域 hook/client，不应建立第二份通用设置对象。

MCP 编辑器有未保存变更保护；离开 MCP 页面或返回工作区前必须确认丢弃。新增带草稿的设置页时，应实现同等的 dirty 生命周期，而不是依赖组件卸载静默丢弃。

## 状态与安全不变量

1. Renderer 中的任何布尔开关都不是权限本身；Electron Main 或 Rust Core 必须再次校验。
2. 跨运行时业务 DTO 使用 `@mycopilot/protocol`；仅属于 Electron Main/Preload 边界的窄形状可定义在 `@mycopilot/host-api`。TypeScript 类型不能替代 Main、Preload 或 Rust Core 边界的运行时校验。
3. 异步状态更新必须绑定当前 conversation/project/Run/request/revision；只检查组件仍挂载不够。
4. Host API 返回的结构化错误应保留 code/data，使 UI 能执行确定的刷新或恢复策略。
5. observer surface 不得获得 interactive mutation callback。
6. 设置和启动遮罩覆盖工作区时，后台 UI 必须 inert，不能仅在视觉上隐藏。
7. 大文本、diff、PDF、图片和流式事件均需先经过领域预算，再进入 DOM/解码器。
8. Automation event/resync 只用于失效通知和排序，不能替代 task、Automation Run、attention 的权威快照。
9. Scheduled 页面中的权限、health、Run 终态和通知状态都不得由 Renderer 文案或本地时钟推断。
10. Automation DTO schema v1、permission mode v1 与 SQLite schema v18 必须分开命名和演进。

## 代码真源

- React 入口：`src/renderer/src/main.tsx`
- Provider 组合：`src/renderer/src/App.tsx`
- 顶层编排：`src/renderer/src/app/AppShell.tsx`
- 工作区/设置隔离：`src/renderer/src/app/AppShellWorkspace.tsx`
- Agent 运行协调：`src/renderer/src/app/useAgentRunLifecycle.ts`
- 启动状态：`src/renderer/src/features/startup/`
- 主题与语言：`src/renderer/src/config/FrontendConfigProvider.tsx`
- Host API 客户端：`src/renderer/src/host/hostClient.ts`
- Storage 投影：`src/renderer/src/features/storage/storageClient.ts`
- 会话能力面：`src/renderer/src/features/chat/ConversationSurface.tsx`
- Automation 页面与状态：`src/renderer/src/features/automations/`
- Scheduled 顶层组合：`src/renderer/src/features/automations/ScheduledPageLayer.tsx`
- Automation Host client：`src/renderer/src/features/automations/automationClient.ts`
- Automation cache/realtime：`src/renderer/src/features/automations/automationCache.ts`、`src/renderer/src/features/automations/automationRealtime.ts`
- Scheduled 布局：`src/renderer/src/features/automations/automationLayout.ts`、`useAutomationLayout.ts`
- 设置导航：`src/renderer/src/features/settings/SettingsPage.tsx`
- 静态依赖规则：`eslint.config.mjs`

## 测试与验证

- 纯状态机、解析、budget 和 reducer 使用 Vitest unit project。
- 真实 React 交互、焦点、生命周期和截图使用 Vitest browser project。
- Electron Main/Preload 契约使用 Node 环境单元测试；跨进程高风险路径另有 Electron fixture 或 managed-playwright E2E。
- Automation 的 cache/realtime/schedule/validation 使用 unit project；drawer、表单、Run 历史、AppShell 导航和布局使用 browser project。

常规验证命令：

```bash
pnpm lint
pnpm typecheck:web
pnpm test:web
pnpm test:automation-core-e2e
```

改动共享协议或 Host API 时同时运行 `pnpm typecheck:node`；改动 Rust Core 行为时运行 `pnpm test:rust`。`test:automation-core-e2e` 启动真实 Core Server，但当前不包含在 `pnpm test:web` 或 `pnpm check` 中，Automation 跨层改动必须显式运行。

## 变更检查表

- [ ] 新代码位于正确层级，未破坏 feature/app/components 导入边界。
- [ ] 新状态有唯一真源，并明确重启、项目切换和会话切换行为。
- [ ] 所有异步响应都有 identity/epoch/revision 防护。
- [ ] observer 与 interactive surface 的能力没有混合。
- [ ] 覆盖层正确设置焦点、`inert`、`aria-hidden` 和恢复行为。
- [ ] 大数据进入 React state/DOM 前执行协议和渲染预算检查。
- [ ] 新 Host API 调用遵守 [IPC 与协议](./ipc-and-protocol.md)。
- [ ] 增加了 reducer/unit 测试以及关键用户交互的 browser 测试。
- [ ] Automation mutation 使用稳定 request identity 或 revision/CAS，迟到响应不会回退共享 cache。
- [ ] Automation event gap、Core Server restart resync、原生通知导航和 dirty drawer 离开保护均有覆盖。
- [ ] Scheduled 表单的 timezone 与权限模式提交语义没有被展示控件或本地默认值悄悄改变。

## 当前限制

- AppShell 仍是较大的跨领域编排点；新增独立领域应优先提取 hook，而不是继续堆叠内联状态。
- 当前没有通用前端路由；设置和右侧栏导航由本地状态机管理。
- 右侧栏页面状态当前不跨应用重启持久化。
- 前端只支持仓库中登记的语言和主题，运行时不能加载第三方主题/语言包。
- 主要桌面流程假设单个主 Renderer；若引入多窗口，状态所有权和 Electron Main 事件订阅必须重新设计。
- Scheduled drawer 宽度只在本次应用会话保留，不写入 UI preferences。
- 保存 Automation 编辑会采用当前电脑 timezone；UI 当前不提供独立 timezone 选择器。
- Automation 的真实 Core Server E2E 是独立命令，默认 `pnpm check` 不会运行。
