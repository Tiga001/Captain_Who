---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-31
---

# 工作区文件与预览

Files 模块提供项目目录树和只读预览。它是 Electron Main 通过 Host API 管理的工作区浏览能力，不是 Agent `read_file`、统一 `FileChange`、附件解析或 Office 文档渲染能力。页面管理见 [右侧栏平台](./right-sidebar.md)，Git 文件跳转见 [Git Review](./git-review.md)。

## 职责边界

| 层                           | 当前职责                                                                                     |
| ---------------------------- | -------------------------------------------------------------------------------------------- |
| Renderer                     | 文件树、筛选、选中项、Markdown source/preview、文本高亮、图片/PDF 显示、copy/reveal 操作入口 |
| Preload/Host API             | 暴露 list/readPreview/copyPath/revealInFolder 四个窄方法                                     |
| Main `WorkspaceFilesService` | 用 project id 解析权威根目录；规范化相对路径；阻止逃逸/symlink；有界读取并分类预览           |
| Rust Core Storage            | 保存 project id 与配置路径，供 Main 解析项目根                                               |

Renderer 不能提交绝对路径，不能从预览结果推导任意文件系统权限。Files 模块只读；复制和 reveal 由 Main 对已验证路径执行。

Agent 的 create/update/delete 与 direct `apply_patch` 现在统一投影为 `FileChange`。它们在聊天 Tool activity/审批对话框中显示 path、状态、统计和按需 diff，并由 Rust Core/Core Server 持有 transaction、observation 和写入授权；不会给 Files panel 增加保存按钮，也不会复用 `WorkspaceFilesService` 作为写入后门。写入完成后，Files 通过下一次 refresh/readPreview 观察磁盘现状，不从 FileChange 卡片乐观修改树。完整写入契约见 [FileChange 子系统](./file-change.md)。

## 目录列举

请求包含 `projectId`、可选相对 `directoryPath` 和 `includeHidden`。Main 通过 Core Server 解析项目路径，并对根与候选执行 `realpath`/containment 检查。

目录列举规则：

- 路径必须是相对路径；拒绝空 segment、`.`、`..`、NUL、POSIX 和 Windows 绝对路径。
- root 请求可使用空字符串，其他文件请求不能为空。
- `.git` 始终不展示。
- `includeHidden` 默认为 true；Renderer 当前明确传 true。
- 顺序为 directory、file、symlink，再按名称进行不区分大小写的数字感知排序。
- 单目录最多返回 20,000 项，并通过 `truncated` 告知截断。
- symlink 作为条目显示，但不能进入目录或预览。

即使词法路径位于根内，也会在非 symlink 情况下对真实路径再次做 containment 检查，防止路径组件解析到项目外。

## 文件树会话

同一 project 的多个 Files 页面共享 `WorkspaceFileTreeSession`，从而复用已展开目录和树模型；只有当前 foreground Files page 是 active consumer。

会话按需加载 root 和已展开目录。每次 refresh 增加 generation、清空 request id/目录状态和模型；迟到请求只有 generation、directory 和 request id 全部匹配才可写入。筛选使用 tree 的 `hide-non-matches` 模式，默认目录折叠、虚拟滚动和 sticky folders。

Files 页面自身使用 `unmount-when-inactive`，但 project-scoped tree session 由 `WorkspaceFileTreeSessions` 资源层管理。预览不会在后台继续读取；再次激活时按 page state 重新加载。

## 预览分类与限制

Main 返回 `metadata.previewKind`：`text`、`image`、`pdf`、`binary`、`too-large` 或 `unsupported`。

| 类型              | 当前 Main 限制 | 处理                                                                      |
| ----------------- | -------------- | ------------------------------------------------------------------------- |
| 文本              | 1 MiB          | UTF-8 或带 BOM 的 UTF-16LE/BE；包含 NUL 或控制字符比例达到 8% 视为 binary |
| 图片              | 12 MiB         | 按登记扩展名识别 AVIF/BMP/GIF/JPEG/PNG/SVG/WebP，返回 base64 与 MIME      |
| PDF               | 32 MiB         | 扩展名 `.pdf` 且前 1,024 bytes 内含 `%PDF-`；返回 `Uint8Array`            |
| 其他/目录/symlink | 无内容         | 返回 unsupported metadata                                                 |

读取实现会按 `limit + 1` 有界扩容并重新核对实际 bytes，不能只相信初始 stat size。

### 文本和源码

Renderer 将文本按行渲染，最多 5,000 行；超过后显示提示，不建立大型 DOM。语言识别与 Git Review 共用 registry，高亮使用异步 Shiki worker；纯文本或超预算内容不启用高亮。换行选项保存在 Files page state。

### Markdown

Markdown preview 使用 `react-markdown` 与 GFM。HTTP/HTTPS 链接阻止页面内导航并交给 Host API 在外部打开；非 anchor 的其他链接被阻止，fragment 可留在文档内。任务 checkbox 被禁用。默认 page state 为 source，source/preview 选择保存在当前页面内存。

### PDF

PDF.js 运行时按需加载，worker 由打包 URL 配置。`isEvalSupported: false`、`enableXfa: false`。页码存入 Files page state；文件 identity 使用 path 与 `modifiedAtMs`，变化后重建 document。PDF 字节仍受 Electron Main 32 MiB 上限约束。

### Office 文件

Word、Excel 和 PowerPoint 在 Files preview 中明确显示“不支持预览”，不会自动调用 Agent 附件解析或 Office rendering pipeline。根 README 中列出的 Office 格式支持属于 Agent 输入/Tool 能力，不等同于 Files 侧栏预览。

## 页面与原生操作

每个文件页使用 `workspace-file:<relative-path>` 作为 resourceKey。同一 workspace session 已存在的文件页会被复用。空 Files 页可承载第一个 transient 预览；打开其他文件会替换当前 transient 页，再次打开同一文件（包括再次点击树中已选中的文件）则把该页稳定为 `tabState: stable`。稳定页不会被下一次预览替换，后续文件会创建新的 transient 页。标签以斜体区分 transient 状态；如果目标已有稳定页，平台激活该页并清理可替换的 transient 来源页；如果目标本身是 transient，也会在激活时升级为 stable。

只有显式的 `tabState: transient` 可被替换。缺失 `tabState` 的旧页面状态按 stable 处理，不能把 optional 字段误解释为 transient 默认值。

每个 workspace 最多 20 个关联文件页，超限按平台规则淘汰；替换 transient 页不新增页面。

page state 保存相对路径、`tabState`、Markdown view、PDF page 和 wrap-lines。项目删除或 workspace 不可用时页面关闭，避免旧 project id 继续发请求。

复制路径与 Finder/Explorer reveal 不把绝对路径先交给 Renderer：Renderer 发送 project id + 相对路径，Main 再解析 exact real path，分别调用 Electron clipboard 或 `shell.showItemInFolder()`。Symlink 不允许执行这两项操作。

## 状态与安全不变量

1. project id 对应的根目录由 Core Server/Rust Core 与 Main 解析，Renderer 不能提供根路径。
2. 所有请求路径必须是规范化相对路径，并在词法与 realpath 两层保持在根内。
3. Symlink 可展示但不可递归、预览、copy real path 或 reveal。
4. `.git` 不进入文件树；Files 不提供写、删除或重命名操作。
5. 文件读取以实际 bytes 进行上限检查，Renderer 还要执行行数/解码预算。
6. 异步目录和预览响应必须绑定 project、path、generation/request id。
7. Markdown 链接不能让主 Renderer 导航到外部或未知协议。
8. FileChange、Git Review 和 Files 可以复用语法/diff 展示组件，但 transaction、snapshot 和 project-relative path identity 不可互换。

## 代码真源

- 协议：`packages/protocol/src/workspaceFiles.ts`
- Host API：`packages/host-api/src/index.ts` 的 `WorkspaceFilesHostApi`
- Main 服务：`src/main/workspaceFiles/WorkspaceFilesService.ts`
- Main IPC：`src/main/ipc/workspaceFilesIpc.ts`
- Renderer panel：`src/renderer/src/features/files/FilesPanel.tsx`
- Tree session：`src/renderer/src/features/files/workspaceFileTreeSession.ts`
- Tree 资源：`src/renderer/src/features/files/WorkspaceFileTreeSessions.tsx`
- Preview：`src/renderer/src/features/files/WorkspaceFilePreview.tsx`
- PDF：`src/renderer/src/features/files/WorkspacePdfPreview.tsx`、`src/renderer/src/features/files/workspacePdfRuntime.ts`
- 右侧栏策略：`src/renderer/src/features/rightSidebar/rightSidebarModules.tsx`
- 右侧栏 reducer 与标签 UI：`src/renderer/src/features/rightSidebar/rightSidebarPlatformState.ts`、`src/renderer/src/features/rightSidebar/RightSidebar.tsx`、`src/renderer/src/features/rightSidebar/RightSidebar.css`

## 测试与变更检查表

关键测试：

- `src/main/workspaceFiles/WorkspaceFilesService.test.ts`
- `src/renderer/src/features/files/__tests__/FilesPanel.browser.test.tsx`
- `src/renderer/src/features/files/__tests__/WorkspacePdfPreview.browser.test.tsx`
- `src/renderer/src/features/rightSidebar/__tests__/rightSidebarPlatformState.test.ts`
- `src/renderer/src/features/rightSidebar/__tests__/RightSidebarWorkspaceLifecycle.browser.test.tsx`
- Tree session 当前由 `src/renderer/src/features/files/__tests__/FilesPanel.browser.test.tsx` 的集成场景覆盖；若拆出状态规则，应在同目录补单元测试。
- 右侧栏 reducer/workspace 生命周期测试

检查项：

- [ ] 新预览类型在 protocol、Host API、Renderer 和 Main size budget 中同步增加。
- [ ] 修改文件打开行为时覆盖 transient 替换、重复点击稳定化、已有 stable 目标去重和 20 页淘汰。
- [ ] POSIX/Windows 路径、`..`、NUL、symlink 和 realpath 逃逸测试齐全。
- [ ] 大小判断不只依赖 stat，读取阶段仍有界。
- [ ] 新渲染器禁用不需要的脚本/eval/导航能力，并有 DOM 预算。
- [ ] project/path 切换会取消或隔离迟到响应。
- [ ] copy/reveal 仍由 Main 解析路径，Renderer 不接收绝对路径。
- [ ] 与 Agent 附件、Office 或 Git Review 的能力区别在用户文案中清晰。
- [ ] FileChange 发生后通过权威 refresh 观察文件，不把聊天 diff 或审批状态写入 Files cache。

## 当前限制

- Files 是只读浏览器，不支持编辑、保存、重命名、删除或创建文件。
- Symlink 不可展开或预览，即使最终目标仍在 workspace 内。
- Office 文档不能在 Files 中渲染。
- 文本预览最多 1 MiB 且最多渲染 5,000 行；图片 12 MiB；PDF 32 MiB。
- 单目录最多 20,000 项；单 workspace 最多 20 个关联文件页。
- 页面和预览选项不跨应用重启持久化。
