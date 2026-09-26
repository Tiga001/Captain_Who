---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-09-26
---

# 对话输入、附件与引用

本文描述 Composer 输入从选择、草稿、排队到 Host 接受和模型消费的当前契约。项目文件树和预览见
[工作区文件](workspace-files.md)，历史持久化见[Trace 与历史归档](../architecture/conversation-trace-and-archive.md)，
模型预算见[上下文管理](../architecture/context-management.md)。

## 输入类型与授权

| 输入              | 持久内容与模型可见内容                                                                               | 权限来源                                         |
| ----------------- | ---------------------------------------------------------------------------------------------------- | ------------------------------------------------ |
| 文本              | 原始用户内容，以及提交时编码的工作区提及                                                             | 普通消息，不单独授予文件权限                     |
| 文件附件          | 原文件在 Host 管理目录；草稿/请求携带 `encoding=managed` 引用；模型得到元数据、`readPath` 和有界内容 | 已登记附件及 Conversation/Agent 树访问范围       |
| Folder Reference  | 名称、绝对目录路径、Host 保存的目录身份；模型可见名称及绝对路径                                      | 用户选择目录后形成的按需读取授权；不扩大写权限   |
| Workspace Mention | `@workspace/<alias>/<path>` Markdown 链接及 Composer 内的结构化选择                                  | 已有项目工作区权限；提及本身不导入文件或增加授权 |
| Skill 选择        | 已选择 Skill 的身份和版本输入                                                                        | 启用、激活和工具权限仍分别检查                   |

附件、文件夹和提及不可仅按名称合并。相同文件名不代表相同内容，相同路径字符串也不证明目录实体仍相同。
Folder Reference 与项目主/辅助根是两种独立来源；选中目录不会将它添加为项目文件夹，也不会递归上传目录。

## 文件导入

原生 picker 由 Main 的 `AttachmentDialogBridge` 打开，并持有选择路径与窗口 owner；拖放或粘贴的
浏览器 `File` 通过受限 Host API 分块导入。两条入口最终都由 Rust Core 创建同一种 managed 引用。

```text
原生选择 / 拖放 / 粘贴
  → beginImport（元数据）
  → appendImport（精确 offset + 有界分块）
  → finishImport（长度、摘要、图片可解码性）
  → managed 引用进入草稿 / 队列
  → Host 接受用户消息或引导，登记附件归属
  → Runtime 按元数据、readPath 和预算构造模型输入
```

当前每块解码后最多 512 KiB。Main 原生读取还会比较前后文件大小和修改时间；Rust Core 检查连续偏移、
声明长度、完整性摘要与元数据一致性。导入完成前不能把原文件包装成已就绪引用。聊天请求和草稿不再保存
整个原文件的 base64，分块传输中的 base64 也不应被误写成完整聊天附件协议。

导入文件位于应用数据根的 `attachment-imports/v1/<importId>/`。manifest v1、opaque import ID 和
SHA-256 共同描述内容；真正读取或复制到附件库时再次校验。已存附件重新加载时复用与内容身份绑定的引用，
避免同一条队列消息因反复加载而得到不同输入身份。复用旧附件用于新消息/引导时，由 Host 重新绑定归属，
不能覆盖原消息的附件行。

### 进度、取消与恢复

- `importProgress` 按 request/attachment 身份投影 `importing`、`complete`、`failed`、`cancelled`。
  Renderer 用会话 scope 和本地 generation 拒绝迟到结果；切换草稿或撤销选择不能把附件插入新会话。
- 失败的原生选择可凭 Main 保留的同窗口选择记录重试；窗口销毁或记录丢失后须重新选择。
- 取消未完成导入会移除其 staging。已完成引用可能已被草稿或待准入动作保存，取消接口不会直接删除它。
- 启动清理仅收集超过 7 天且未被 Composer 草稿/队列、pending action 或人机交互暂停引用的旧导入。
  遇到不能解析的持久 JSON 时停止该清理，不能猜测哪些引用可删除。
- 原文件导入大小、单次 Tool 文件输入、图片视觉预算和模型上下文预算是不同限制，不能用其中一个值
  宣称所有附件均可完整交付模型。

## 附件的模型投影

Runtime 先解析已登记的 `readPath`，不会凭模型自报文件名读取本机路径。当前普通非图片附件只有在原文件
不超过 256 KiB 且存在对应只读解析器时才尝试预读，所有预读文本合计最多 40,000 字符；较大文件保留
元数据和按需读取提示。PDF 不在发送时自动提取正文，应通过当轮实际可用的工具或 Skill 处理。

图片保留原件，模型使用受限衍生图：最长边不超过 2,048 像素，必要时继续缩小以满足单图视觉字节预算；
当前一次附件上下文的衍生图片合计预算为 16 MiB。解码层还限制像素量和内存。缩略图用于 UI，不能代替
模型交付或完整原件，也不能因缩略图可显示就宣称原图已进入模型请求。预算用尽或无法生成视觉输入时，
上下文保留对应状态和读取入口。

这些值的真源是 `runtime/attachments.rs`、`file_input.rs` 与 `file_input/image_delivery.rs`；
修改时要同时检查预处理、`read_image`、历史重放与 Renderer 缩略图的消费者边界。

## 文件夹引用

Main 对选择/拖入的目录做规范化与存在性检查；Rust Core 保存 `rootPath`、目录 identity 和可用性。
每次构建 authority 或解析子路径时重新核对目录实体，拒绝路径逃逸及不允许的 symlink 组件。目录被移动、
删除或替换时保留历史引用并报告不可用，不能把相同字符串静默重新绑定到另一个目录。

模型会看到选定目录名称和绝对路径，可使用 `workspace_map`、`search_files`、`search_code`、`read_file`
等按需浏览读取；`rootIdentity`、`status` 是内部绑定信息，不注入模型正文。读取授权不改变本 Run 的全局
写权限，外部目录的 FileChange 或命令副作用仍需各自权限判断。文件夹下存在 `AGENTS.md` 不意味着自动进入
项目指令层；工作区指令的发现范围以[冻结工作区根规则](workspace-instructions.md)为准。

文件夹引用与普通附件分别持久化到消息、引导和草稿的对应字段；Fork 保留原身份，即使当前目录不可用。
持久解析器兼容历史目录 identity 的 snake_case 字段并统一写为 camelCase；这仅是字段读取兼容，不能弱化
目录复验。用户消息、队列和已应用引导都展示同一引用，UI 展示名不能成为新授权。

## 工作区提及与统一添加菜单

`+` 和 `@` 入口共享文件/文件夹搜索与 Skill 选择。当前项目内搜索由 Main 的 `searchMentions` 完成，
返回 folder ID、alias、相对路径及展示信息；具体遍历上限、排除目录和 symlink 规则见[工作区文件](workspace-files.md)。
检索结果不是全仓内容，也不是文件导入结果。

Composer 暂存结构化 `workspaceMentions`，提交时通过 `buildMessageContentWithWorkspaceMentions` 转为
Markdown：

```markdown
[notes.md](@workspace/main/notes.md)
```

文件夹用末尾 `/` 标识。历史消息读取这些逻辑
链接，点击文件或目录进入现有 Files 预览/定位流程。它们不携带本机绝对路径，不绕过项目 alias、folder
成员或历史文件打开时的校验。`@` 提及与已选择的 Folder Reference 不可互相转换授权。

## 草稿、下一轮队列与运行中引导

只有附件、文件夹或工作区提及而没有文本，也可构成有效用户输入；不要生成伪用户正文来通过旧的非空检查。
模型所需的附件/文件夹上下文由 Host 另行组装。三种输入生命周期必须分别处理：

| 状态               | 谁拥有事实                                 | 失败或重试规则                                                            |
| ------------------ | ------------------------------------------ | ------------------------------------------------------------------------- |
| Composer 草稿      | Renderer 编辑，持久保存草稿引用            | 提交按 `updatedAt` 消费原快照；不得清空之后输入的新内容                   |
| 下一轮队列         | 持久排队项保存模型、权限、项目、附件和引用 | Host 接受后移除队首；忙碌、审批或模型切换等待对应状态变化，不自旋重发     |
| 已准入的运行中引导 | Rust Core 的 guidance 记录、状态及应用事件 | 按稳定 client identity 幂等；未应用输入仍可恢复，不把它误记为已被模型看到 |

普通提交失败时恢复用户输入，并保留等待期间新编辑的内容；Host 已接受的消息正文、附件和历史事实不能被
迟到的 Renderer 全量保存改写。排队项拥有自己的设置快照，不随输入框下一轮模型或权限选择变化而被覆盖。
自动发送由 Host 完成/可发送状态和用户已启用的队列意图驱动；账号切换使当前内存自动发送意图失效，持久
队列仍可见。模型转换和手动压缩占用时，等待已有流程结算。

运行中引导可包含纯附件或文件夹引用。应用事件将文字、附件和引用定位到本轮 Trace；排队显示、恢复输入及
历史卡片必须保持同一内容身份。具体幂等结算与恢复见
[Core Server](../architecture/core-server.md)及[Trace 与历史归档](../architecture/conversation-trace-and-archive.md)。

## 代码与测试真源

- 协议：[attachments.ts](../../packages/protocol/src/attachments.ts)、[Host API](../../packages/host-api/src/index.ts)。
- 原生导入：[AttachmentDialogBridge.ts](../../src/main/attachments/AttachmentDialogBridge.ts)。
- 输入与进度：[chatAttachments.ts](../../src/renderer/src/features/chat/chatAttachments.ts)、[useAttachmentImports.ts](../../src/renderer/src/features/chat/useAttachmentImports.ts)。
- 提及：[workspaceMentions.ts](../../src/renderer/src/features/chat/workspaceMentions.ts)、[WorkspaceFilesService.ts](../../src/main/workspaceFiles/WorkspaceFilesService.ts)。
- 草稿与提交：[composerSubmission.ts](../../src/renderer/src/app/composerSubmission.ts)、[useAppShellMessageSubmission.ts](../../src/renderer/src/app/useAppShellMessageSubmission.ts)。
- 持久导入：[attachment_imports.rs](../../crates/core/src/storage/service/attachment_imports.rs)、[attachments.rs](../../crates/core/src/storage/service/attachments.rs)。
- 文件夹：[folder_input.rs](../../crates/core/src/folder_input.rs)、[Tool context](../../crates/core/src/tools/context.rs)。
- 模型投影：[runtime/attachments.rs](../../crates/core/src/runtime/attachments.rs)、[image_delivery.rs](../../crates/core/src/file_input/image_delivery.rs)。
- Host 测试：[attachmentDialogBridge.test.ts](../../src/main/core/attachmentDialogBridge.test.ts)。
- Rust 测试：[attachment_imports.rs](../../crates/core/src/storage/service/tests/attachment_imports.rs)、[steering.rs](../../crates/core-server/src/application/agent/tests/steering.rs)。
- Renderer 测试：[AttachmentImports.browser.test.tsx](../../src/renderer/src/features/chat/__tests__/AttachmentImports.browser.test.tsx)、[GuidanceQueueAttachments.browser.test.tsx](../../src/renderer/src/features/chat/__tests__/GuidanceQueueAttachments.browser.test.tsx)、[workspaceMentions.test.ts](../../src/renderer/src/features/chat/__tests__/workspaceMentions.test.ts)。

## 变更核对

- 导入完成前的取消、切会话和迟到事件不会污染新草稿；重试不制造重复附件。
- managed 引用需同时核对元数据与内容完整性；原件、模型衍生图、UI 缩略图分开保存和投影。
- folder grant、项目冻结根与工作区提及分别校验；目录替换、重启和 Fork 不丢失原身份。
- 纯附件/文件夹输入覆盖首次发送、队列、运行中引导、历史重载和 Fork。
- 提交拒绝后输入可恢复；Host 接受后 Renderer 不能回写旧正文；自动发送不能绕过账号/许可准入。
