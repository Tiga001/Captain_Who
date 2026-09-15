---
status: current
audience: developers
owner: engineering
last_verified: 2026-09-16
---

# 右侧栏平台

右侧栏是 Renderer 内承载长生命周期 Tool surface 的模块化多页面平台。页面和模块状态归平台所有，不归工具栏或模块选择器所有。整体前端边界见 [前端架构](../architecture/frontend.md)。

## 职责边界

平台负责：

- 模块注册、可用性和入口展示。
- 页面创建、复用、激活、更新、关闭和关联页面跳转。
- 单实例/多实例规则、工作区绑定、孤儿页面处理和数量上限。
- 页面挂载策略以及 foreground/background/dormant 活动信号。
- 将 AppShell 提供的 workspace、capability、浏览器 automation request 和 Agent observer 上下文注入模块。

平台不负责：

- Terminal、Browser、Files、Git Review 或 Agent Center 的领域 I/O。
- 把选中页面当成 Electron Main capability 或 Agent 授权。
- 持久化业务记录。当前页面栈和局部页面状态只存在 Renderer 内存。

## 核心模型

`RightSidebarModuleDefinition` 是模块契约，声明：

- `id`、图标、翻译标题和 `surfaceKind`。
- `createPage` 与 `render`。
- `retention`：`keep-alive` 或 `unmount-when-inactive`。
- `instancePolicy`：`multiple`、`single` 或 `single-per-workspace`。
- `contextBinding`：`global`、`pinned-to-creation-workspace` 或 `follow-workspace`。
- 是否要求 workspace/capability。
- capability 不可用或 workspace 被删除时关闭还是保留页面。
- 关联页面在每个工作区的数量上限。

`RightSidebarPage` 保存平台身份和轻量展示状态：page/module id、title、icon、resourceKey、workspace key/path/session key、创建时捕获的真实 `projectId`（无持久项目时为 null），以及模块特定的 tagged `moduleState`。`workspaceKey` 可能回退为路径、名称或 `home` 等 UI 标识，只有 `projectId` 代表持久项目身份；模块的后端 project 参数只从 `projectId` 派生，不得从 UI key 推断。页面状态当前支持 browser `surfaceId`/逻辑 URL/viewport、workspace file preview、最近一次 Turn 的 Git 导航和 Agent Center list/detail。Browser 的 `surfaceInstanceId`、state revision、真实 guest URL 和下载路径不属于 Renderer page state。

## 当前模块矩阵

| 模块         | Surface | 实例策略 | 上下文绑定                                      | Retention             | 可用性/清理                                                                              |
| ------------ | ------- | -------- | ----------------------------------------------- | --------------------- | ---------------------------------------------------------------------------------------- |
| Terminal     | React   | multiple | 固定到创建时工作区                              | keep-alive            | 无 workspace capability；页面关闭时组件卸载并终止 PTY                                    |
| Browser      | webview | multiple | global                                          | keep-alive            | 页面跨项目保留；关闭是 guest/surface 清理边界                                            |
| Files        | React   | multiple | 固定到创建时工作区                              | unmount-when-inactive | 要求 workspace；项目删除或不可用时关闭；每个 workspace 最多 20 个有 resourceKey 的文件页 |
| Git Review   | React   | single   | 跟随当前工作区                                  | keep-alive            | 要求 `git-repository` capability；不可用时关闭                                           |
| Agent Center | React   | single   | 跟随当前工作区，并由活动根 Agent 会话进一步限定 | keep-alive            | 只有活动根 Agent 的授权树包含子 Agent 时动态注册                                         |

静态注册表是 `RIGHT_SIDEBAR_MODULES`；Agent Center 由 AppShell/RightSidebar 根据当前 collaboration snapshot 动态追加，不能无条件放入没有子 Agent 的静态注册表。

## 页面状态机

`useRightSidebarPlatform` 包装纯 reducer，公开打开模块、确保页面、打开关联页、激活、更新、关闭和同步上下文等操作。

### 打开与实例复用

- `multiple` 每次创建新页。
- `single` 在整个页面栈复用第一个同模块页面。
- `single-per-workspace` 按 `workspaceSessionKey` 复用；当前注册模块尚未采用该策略。
- 创建和激活在同一个 reducer action 中完成，避免中间出现无活动页状态。

### Composer 工作区命令

Composer 的 `/` 菜单通过 AppShell 的显式打开操作导航，不能复用会关闭已展开面板的 toggle。`useRightSidebarModules` 和 `useRightSidebarModuleAvailability` 同时服务菜单与页面平台，避免另建审阅/子 Agent 资格规则。不可用的右栏命令隐藏，Git 检查中显示禁用状态；这些导航不要求 Agent 空闲。

| 命令                  | 目标与复用规则                                                                                      |
| --------------------- | --------------------------------------------------------------------------------------------------- |
| 打开终端 `/terminal`  | 展开底部栏，每次创建并选中新终端，保留现有会话与面板高度。窗口不足以容纳底部栏时禁用。              |
| 内置浏览器 `/browser` | 展开右侧栏，每次新建空白浏览器页，保留其他标签。沿用浏览器内部 bootstrap，不导航外部地址。          |
| 查看修改 `/review`    | 展开并选中现有 Git Review，保留当前审阅目标与局部状态；首次打开使用模块默认的未提交修改。           |
| 子 Agent `/agents`    | 展开并复用 Agent Center，回到当前根聊天的子 Agent 列表。只在同一根聊天的授权树存在子 Agent 时显示。 |

导航请求包含递增 `requestId`、工作区 key/path 和 conversation id。消费方校验当前上下文并去重；旧项目/聊天请求被消费后丢弃，切回原上下文也不重放。底部栏将外部终端请求与首次打开的默认终端初始化合并，React effect 重放不额外创建终端。命令只消费 Slash 查询，不提交模型消息或清除附件。

### 关联页面

模块可以通过 `onOpenPage` 打开同模块或跨模块页面。`resourceKey` 与目标 module、`workspaceSessionKey` 一起去重。文件去重键包含来源 folder id、历史 assistant-message id（如有）和源内相对路径；同一来源重复打开会激活已有页，不同根的同名文件分别保留。

关联页 disposition 当前支持：

- `new-page`：创建独立页面；
- `reuse-source-if-empty`：仅复用尚未绑定资源的目标模块空页；
- `preview`：用于 Files 的临时预览语义。

Files 第一次打开文件时复用同一 workspace session 的空 Files 页（没有空页则创建一页），并将其标记为 `tabState: transient`，标签使用斜体。继续打开其他文件会替换当前 transient 页，而不是不断新增标签；再次打开同一个 transient 文件会把它升级为 `stable`。稳定页不会再被后续预览替换，下一份文件使用新的 transient 页。若目标文件已经有页面，平台会激活已有页，并在需要时丢弃作为来源的 transient 页；对已有 transient 目标的重复打开会将其稳定化。

`FilesPanel` 对已选中文件的再次主键点击也会重新提交打开请求，因此用户可以显式固定当前预览。达到 `maxRelatedPagesPerWorkspace` 时，平台优先淘汰非活动、非受保护的最早关联页，然后再考虑其他页；替换现有 transient 页不消耗新名额。Files 的当前上限为 20。

### 多文件夹选择

三个选择入口仅在项目配置了多个文件夹时显示，彼此独立，不改变项目主文件夹或 Agent 的冻结工作区。

- Terminal 默认在主文件夹启动。首次真实输入之前，提示符下面显示源目录 alias、绝对路径提示及 Option/Alt + 数字快捷键。选择后 Host 根据创建时冻结的 folder id 和目录实体校验路径，向同一 PTY 发送 shell 正确引用的 cd 命令；不重建会话，不修改标签或假定 cd 成功。点击选择或键盘、粘贴、IME 输入永久收起该层；自动终端协议回复不会收起。配置变化不自动切换或重启已有终端。
- Files 在搜索栏上方显示文件夹下拉，默认为主文件夹，显示 alias 与选中标记。选择只改变目录树；每个源保留独立搜索和展开状态，已有预览和标签继续绑定原 folder id。树缓存按 project、folder id 和该源 revision 区分；源移除或改绑后不能回退到另一个根的同名文件。历史页额外携带 assistant-message id，沿用原 Run 的目录快照。
- Git Review 在范围选择器左侧显示仓库下拉。项目任一源为 Git 仓库即可打开审阅；多文件夹项目即使只有一个 Git 源，也显示下拉。只有 LastTurn 支持“所有仓库”，按同一次 Run 聚合、按来源分组并合计统计，保持只读。其他范围一次选择一个源，离开“所有仓库”时恢复记住的单源，否则选择主 Git 源或第一个可用 Git 源。单源路径范围保留 Git 子目录限制；更换源会失效旧 diff、内容、分支和提交查询。

### 关闭与上下文同步

关闭活动页后，优先激活原位置的下一页，再退到前一页。同步工作区时平台会：

- 删除注册表中已不存在的模块页。
- 根据已知项目 key 关闭 orphaned workspace 页。
- 根据 capability 和 `unavailablePagePolicy` 关闭不可用页。
- 消除违反 `single` 策略的重复页。
- 将 `follow-workspace` 页面重绑定到当前 workspace。
- 若活动页被移除，选择相邻仍存页面。

`workspaceSessionKey` 不只使用 project id，还表达当前 workspace session 身份。页面的 React `key` 包含它，确保 workspace 重新绑定时需要重建的领域组件不会保留旧项目状态。

## 活动与挂载生命周期

页面活动分为：

- `foreground`：右侧栏可见、文档可见且页面被选中。
- `background`：右侧栏和文档可见，但页面未选中。
- `dormant`：右侧栏隐藏或整个 document 不可见。

`RightSidebarPageStack` 始终为页面保留 frame：

- `keep-alive` 页面即使未选中也保持模块挂载，只以 `aria-hidden` 和 CSS 隐藏。Browser 的导航/cookie/guest、Terminal 的 PTY、Git Review 和 Agent Center 的内存状态因而保留。
- `unmount-when-inactive` 页面只在选中时渲染模块。Files 重新激活后按 page state 重新加载预览。

活动信号用于降低后台工作，不是授权信号。模块必须在真正关闭/卸载时释放订阅、计时器和外部资源；仅变成 `background` 或 `dormant` 不等于销毁。

Terminal 创建区分 `created` 与正常 `cancelled`，项目加载、PTY 启动和 utility 进程的真实错误仍通过拒绝返回。关闭、导航或 Renderer 销毁会立即撤销正在创建的会话；项目目录尚未加载完成时不再创建 PTY，已发送的创建则清理该次实例。Main 为每次创建分配独立的内部 PTY id，并将事件回译为 Renderer 会话 id，避免旧响应、输出或退出事件影响复用同一外部 id 的新实例。Renderer 的异步回调和清理同样绑定各自的 effect 实例，开发模式下的 effect 重放不能清空新终端的输入、尺寸或来源目录引用。

## Browser surface 交接

Browser page 的 `surfaceId` 稳定地由 page id 派生，`surfaceInstanceId` 则由 Main 为某一次精确 guest incarnation 生成。AppShell 接收 Main 的 surface command，平台打开/激活相应 Browser 页面，`BrowserPanel` 完成 guest 注册后回传精确 request/surface/instance/viewport。地址栏动作由 Main 执行；面板只接受匹配 instance 且 `stateRevision` 不倒退的 logical URL/title/favicon/loading/error/crash projection。

下载中心是 Browser toolbar 中 portal 到 `document.body` 的 popover，不是独立右侧栏 page；切换 Browser 页、关闭/失焦和锚点销毁时必须收口。它只持有 path-free live snapshot，pause/resume/cancel/reveal/copy/remove 都通过 Host API 返回 Main。Browser menu 可导航到独立 Browser Settings 的 history/downloads 子视图，不把它们注册成 MCP 页面或右侧栏模块。

关闭 Browser page 必须同时撤销 exact surface command、selection、instance、guest subscription 和 Main surface；仅变为 background/dormant 不清理。新建同名 page/`surfaceId` 后的 guest 是新 incarnation，旧 state/error/download callback 不得写回。

平台只协调 UI 页面；它不持有 WebContents、Target 或 CDP 权限。完整协议见 [浏览器与自动化](./browser-automation.md)。

## 根 Agent 作用域的 Agent Center

Agent Center 是当前根 Agent Conversation 的只读子 Agent 索引：

- 首页将当前树分为 active 与 non-active，按最近活动排序，只显示可展示的任务、状态、模型和时间信息。
- 选择 Agent 后切换到 detail state，通过 AppShell render contract 复用 `ConversationSurface` 的 observer 模式。
- 同一项目内切换根 Agent 也会重置 detail 到新根 Agent 的列表；快照不匹配时 fail closed；没有子 Agent 时移除模块。
- Observer 没有 composer、send、edit、retry、fork、stop/guide 或 approval 控件。Core Server/Rust Core 仍会校验精确根 Agent 与子 Agent Conversation，因此隐藏控件不是授权边界。
- 共享会话 surface 在 280 px 侧栏最小宽度下使用局部布局覆盖，不维护第二套聊天实现。

AppShell 是活动根 Agent collaboration store 的唯一所有者。该 store 从持久事件序列重放并检查 gap；当前实现发布有界的最近 2,048 条语义活动窗口。只有能解析到持久根 Agent assistant-message/trace-boundary anchor 的事件进入根 Agent chat 时间线；父 Agent 最终回复开始流式输出时，该回复的 collaboration timeline 即冻结。之后的子 Agent 活动仍可更新 Agent Center，但不能追写已结算父消息。Agent Center 当前状态、observer live envelope 和根 Agent chat 历史是三种不同投影，不能互相推导。

Observer 更新必须绑定根 Agent、子 Agent、Conversation、Run 和 assistant-message 身份。hydration revision 会在 gap、restart resync 或 reload 后失效全部 observer；子 Agent A 的迟到响应不能显示在子 Agent B 下。live observer envelope 只是有界、进程内的低延迟覆盖，持久 Conversation 和 collaboration event log 在恢复后重新成为权威。

Agent Center 的设置入口打开通用 Agent template 设置页。模板定义现在是 workspace-wide library，CRUD 不绑定单个 project；每个模板以独立 assignment 关联零到多个 project，只有分配给当前 project 且 enabled 的模板可用于该树。模板保存精确 `model_config_id`；已删除/禁用模型必须明确替换后才能保存或重新启用。表单的 description/instructions 提供 guidance-oriented placeholder，但 placeholder 不会写入空字段。模板编辑或 assignment 变化只影响后续 Agent，现有 Agent 显示创建时快照。

持久化的是 Agent、状态、模板、审批、子 Agent Conversation 和语义事件。Agent Center 当前打开页/detail、折叠和滚动位置不会跨完整 Renderer reload 恢复。当前没有子 Agent 删除、图画布或跨根 Agent dashboard。

## 增加 React 模块

1. 扩展 `RightSidebarModuleId` 和需要的 tagged `moduleState`。
2. 在 registry 中声明完整 module definition，不在 `RightSidebar.tsx` 写 module-specific 分支。
3. 明确实例策略、workspace 绑定、retention、不可用与 orphan 策略。
4. 需要 workspace 能力时先扩展 capability 类型和 AppShell 探测逻辑。
5. 关联资源使用稳定且不含秘密的 `resourceKey`，并设置合理上限。
6. 模块根据 activity 降低后台工作，在关闭/卸载时释放资源。
7. 增加 reducer 单元测试、workspace 生命周期和浏览器交互测试。

## 增加 Webview 模块

1. 除上述步骤外，通过共享 `WebviewSurface` 渲染，不在其他位置直接创建 `<webview>`。
2. 需要隔离 cookie 时使用独立持久 partition。
3. 在 Main 的 `MANAGED_WEBVIEW_POLICIES` 登记 partition 和协议 allowlist。
4. 仅在目标链接应回到当前 surface 时启用 same-surface popup 处理；真实 Electron popup 仍由 Main 拒绝。
5. Main 必须删除 guest preload、强制 sandbox/context isolation、拒绝权限并限制导航。
6. Host API 只能暴露窄能力，guest 永远不能收到主应用 API。

## 状态与安全不变量

1. 平台 reducer 是页面栈和活动页的唯一写入口。
2. `pageId` 是 Renderer 页面身份，`resourceKey` 是同 workspace 资源去重键；二者不能代替 Electron Main 或 Rust Core capability。
3. `follow-workspace`、pinned 和 global 三种绑定不可凭模块当前 props 临时猜测。
4. capability 必须来自权威探测，并绑定对应 `contextKey`；旧项目结果不得作用于新项目。
5. 未选中页面必须 `aria-hidden`；覆盖/隐藏整个侧栏时活动变为 dormant。
6. Browser guest 必须通过 Electron Main 身份校验，Agent observer 必须通过 Core Server/Rust Core 会话树校验；UI 页存在不代表拥有访问权。
7. Browser logical state 必须匹配 exact instance/revision；download center capability 不等于路径已经暴露给 Renderer。
8. `workspaceKey` 可能包含 `home` 等 UI 回退，只有 `projectId` 是持久项目身份；无项目终端等场景不得从 UI key 推断或凭空生成项目。

## 代码真源

- 类型契约：`src/renderer/src/features/rightSidebar/rightSidebarTypes.ts`
- 模块注册：`src/renderer/src/features/rightSidebar/rightSidebarModules.tsx`
- 纯 reducer：`src/renderer/src/features/rightSidebar/rightSidebarPlatformState.ts`
- 标签与组合 UI：`src/renderer/src/features/rightSidebar/RightSidebar.tsx`、`src/renderer/src/features/rightSidebar/RightSidebar.css`
- Hook：`src/renderer/src/features/rightSidebar/useRightSidebarPlatform.ts`
- Page stack：`src/renderer/src/features/rightSidebar/RightSidebarPageStack.tsx`
- Activity：`src/renderer/src/features/rightSidebar/rightSidebarActivity.ts`
- Availability：`src/renderer/src/features/rightSidebar/rightSidebarModuleAvailability.ts`
- Workspace identity：`src/renderer/src/features/rightSidebar/rightSidebarWorkspace.ts`
- Runtime 注入：`src/renderer/src/features/rightSidebar/RightSidebarRuntimeContext.tsx`
- Webview host：`src/renderer/src/features/rightSidebar/surfaces/WebviewSurface.tsx`
- Agent Center：`src/renderer/src/features/agentCollaboration/AgentCenterPanel.tsx`
- Agent Template 设置：`src/renderer/src/features/settings/pages/AgentTemplatesSettingsPage.tsx`
- Browser panel/download center：`src/renderer/src/features/browser/BrowserPanel.tsx`、`BrowserDownloadCenter.tsx`

## 测试与变更检查表

主要测试：

- `rightSidebarPlatformState.test.ts`
- `rightSidebarActivity.test.ts`
- `RightSidebarWorkspaceLifecycle.browser.test.tsx`
- `FilesPanel.browser.test.tsx`
- `BrowserPanelLifecycle.browser.test.tsx`
- `BrowserSurfaceRightSidebar.browser.test.tsx`
- `BrowserDownloadCenter.browser.test.tsx`
- `AgentCenterRightSidebar.browser.test.tsx`

变更前后检查：

- [ ] module definition 的所有策略字段均明确，没有依赖默认猜测。
- [ ] project 切换、项目删除、capability checking/unavailable 均有确定行为。
- [ ] keep-alive 与 unmount 行为有资源释放测试。
- [ ] 单实例、resourceKey 去重、transient/stable 预览、重复点击固定、斜体标签、页面上限和关闭后的选中项有 reducer 与 Browser 测试。
- [ ] 键盘、焦点、`aria-hidden` 和 dormant 行为已验证。
- [ ] Webview 权限仍由 Electron Main 校验，observer 权限仍由 Core Server/Rust Core 校验。
- [ ] Browser state、错误/crash、下载 popover 和关闭清理均绑定当前 instance/revision，旧 guest 不会复活页面。
- [ ] 更新了本模块矩阵和相关子系统链接。

## 当前限制

- 页面栈、活动页和模块局部状态不跨完整 Renderer reload 持久化。
- 仅有 `git-repository` 一种通用 capability id。
- 当前没有页面拖拽排序、显式 pin/unpin 控件或跨窗口迁移；Files 的 repeated-open `stable` 状态只是本次 Renderer 生命周期内的预览固定语义。
- Files 的关联页面上限按 workspace session 管理，不是全局最近文件列表。
- Agent Center 只覆盖活动根 Agent，不提供跨根 Agent 聚合。
