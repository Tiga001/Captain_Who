# Captain Who

简体中文 | [English](README.en.md)

**在本地项目里，让 AI 帮你查资料、做文档、改代码。**

Captain Who 是一款本地优先的桌面 AI 工作助手。接入自己的模型 API，用自然语言说明任务；它可以读写文件、运行命令、操作内置浏览器，也可以把任务交给多个智能体并行处理。对话、文件、终端和执行记录放在同一个工作界面里。

[官网与下载](https://captainwhoagent.com/) · [界面与功能](#界面与功能) · [快速开始](#快速开始) · [从源码运行](#从源码运行) · [使用文档](public-docs/README.md)

![制作浙江大学介绍 PPT 时，四个子智能体正在分别检索资料。](assets/screenshots/parallel-research.png)

制作一份大学介绍 PPT，将校史、科研、人物和排名资料交给四个子智能体并行检索。

## 下载与项目状态

想直接使用，可以到 **[captainwhoagent.com](https://captainwhoagent.com/)** 下载 macOS 安装包，目前支持 **macOS 12 及以上的 Apple Silicon Mac**。

这是一个个人开发项目，目前没有时间完成 Windows 版本，暂不提供 Windows 安装包，也没有确定的发布时间。

本仓库是 Captain Who 的真实开发仓库，会持续更新，采用 [Apache-2.0](LICENSE) 开源许可证。开发代码可能领先于官网安装包，正式版本信息见[发行与升级](public-docs/releases/README.md)。

> 启动 Agent 任务需要登录 Captain Who 账户、通过软件许可校验，并配置自己的模型 API。软件不包含模型调用额度。

## 界面与功能

以下截图来自实际使用，按场景整理；模型名称、配置和界面以你安装的版本为准。截图中的完全权限不代表推荐设置，首次使用建议保留默认权限。

### 处理文件与代码

围绕本地文件夹工作，在对话旁查看文件、核对代码改动，并使用内置终端。

![项目对话、Python 文件差异和内置终端并排显示。](assets/screenshots/code-review-terminal.png)

查看一个二分查找脚本的新增代码，底部同时打开项目终端。

<details>
<summary>更多：项目对话、文档预览与快捷操作</summary>

![项目中的对话页面，输入框下方显示模型和权限选择。](assets/screenshots/workspace-chat.png)

在项目中发起对话，让助手介绍当前能做的事，并查看所选模型与权限。

![对话旁预览 Markdown 文档，并展开单条回复的用量统计。](assets/screenshots/markdown-preview-tokens.png)

一边讨论文档内容，一边预览 Markdown 文件，并查看该次回复的 Token 用量与缓存命中率。

![输入斜杠后展开模型、压缩上下文和对话管理菜单。](assets/screenshots/chat-commands.png)

输入 `/` 打开快捷菜单，切换模型、压缩上下文或管理当前对话。

</details>

### 并行研究与生成素材

子智能体可以分别检索、撰写和审查；文档、表格、演示文稿、PDF 与图片任务可配合对应 Skill 完成。

![生成的校园插画出现在对话中，右侧列出子智能体进度。](assets/screenshots/generated-artwork.png)

为 PPT 生成校园配图，并在用户补充要求后增加玉泉校区的专项检索。

### 浏览器与人机协作

Agent 可以读取和操作应用内网页，遇到需要你判断或亲自操作的环节，通过交互卡片请求协助。

![英文对话与内置浏览器并排，网页打开了联系窗口。](assets/screenshots/browser-inspection.png)

让助手查看内置浏览器中的官网页面，说明当前页面和弹窗里有什么。

<details>
<summary>更多：登录接管与交互问答</summary>

![浏览器停在网站登录页，对话中显示请求用户协助的交互卡片。](assets/screenshots/browser-login-handoff.jpg)

检查邮箱时遇到登录页，助手请用户在浏览器中完成登录，再继续检查邮件。

![对话中出现带选项和自定义答案的多题交互卡片。](assets/screenshots/interactive-questions.png)

通过逐题选择或填写答案，完成一组用于演示交互能力的 MBTI 风格问答。

</details>

### Skills、MCP 与子智能体模板

Skill 保存任务方法和配套资源，MCP 连接外部工具，子智能体模板保存可复用的分工与模型设置。

![技能安装窗口提供 GitHub 和本地文件夹两个来源。](assets/screenshots/install-skills.png)

从公开 GitHub 仓库或本地文件夹安装 Skill，扩展助手的做事方法。

<details>
<summary>更多：MCP 连接与专家模板</summary>

![MCP 设置列出本地服务器及其开关与连接状态。](assets/screenshots/mcp-servers.png)

管理本地 MCP 服务器，确认文件系统工具已就绪，其余连接保持关闭。

![子智能体设置中启用了一个视觉审查专家模板。](assets/screenshots/subagent-templates.png)

启用“视觉审查专家”模板，为它指定模型并分配到项目。

</details>

### 模型、权限与用量

模型服务由你选择，文件访问范围和审批规则由你设置；用量面板记录模型调用与费用估算。

![权限设置展示默认、完全和自定义模式，以及读写范围与审批选项。](assets/screenshots/permissions.png)

设置可用的权限模式，并调整自定义模式的文件范围和审批规则。

<details>
<summary>更多：模型配置、用量统计与外观</summary>

![模型编辑页面显示模型 ID、上下文窗口、价格、图像输入和 API 配置。](assets/screenshots/model-configuration.png)

配置模型的 API 连接、上下文窗口、图像输入能力和用于估算费用的单价。

![用量面板按日期展示模型输入、输出、缓存和费用统计。](assets/screenshots/usage-costs.png)

按模型查看近七天的调用用量、缓存命中率和估算费用。

![外观设置展示主题选择、代码差异配色预览和显示偏好。](assets/screenshots/appearance.png)

选择深色主题，预览代码差异配色，并调整字体平滑与侧边栏效果。

</details>

### 协作界面的演进

当前开发版使用“组织”：多个独立智能体按职责交流，通过邮件传递任务和结果。它与普通对话中临时派出的子智能体不同，具体设计见[组织协作开发文档](docs/subsystems/workflow-authoring.md)。开发版能力不代表官网安装包已提供。

<details>
<summary>早期工作流界面（历史截图，非当前组织界面）</summary>

![早期工作流画布连接文案协调者、两位编辑和审阅者。](assets/screenshots/visual-workflow.png)

早期版本用连线展示文案协调、双人编辑和审阅流程；当前组织协作已不再采用这套连线机制。

</details>

此外，还支持在“已安排”中设置周期任务，查看每次执行记录与通知；定时执行需要应用保持运行。详见[定时任务](public-docs/user/capabilities/automations.md)。

## 快速开始

1. 从[官网](https://captainwhoagent.com/)下载安装，登录账户并确认软件许可可用。
2. 打开 **设置 → 配置**，填写模型 API URL 和 Token；在 **管理模型**中添加实际模型 ID，选择匹配的服务商配置，再启用模型。新安装的模型列表为空，需要自行配置。
3. 新建对话，选择一个本地文件夹作为项目，选择模型并保留默认权限。
4. 说明任务和输出位置；需要时从输入框 **“+” → 技能**选择对应 Skill，核对审批与最终文件。

可以从一个简单任务开始：

```text
请阅读这个文件夹里的资料，整理一份摘要，保存为 summary.md。
列出主要结论、对应来源和仍需确认的问题，不要修改原始文件。
```

模型接入支持 OpenAI-compatible、Anthropic-compatible、DeepSeek 和 Moonshot 配置。联网搜索与图片生成需要另行配置对应服务，不包含在聊天模型配置中。

[安装与首次启动](public-docs/user/getting-started/installation.md) · [连接模型](public-docs/integrations/model-provider-integration.md) · [第一个任务](public-docs/user/getting-started/first-task.md)

## 数据与使用边界

- **本地优先不等于离线。**登录账户不会把项目、对话和模型密钥同步给 Captain Who 账户服务；模型、搜索、图片、网站及 MCP 仍会接收完成任务所需的数据。详见[数据与权限](public-docs/security/data-and-permissions.md)。
- **权限不是操作系统沙箱。**默认模式也不是只读模式；请检查文件写入、命令执行和外部操作的审批。停止任务不会撤销已经完成的操作。
- **输出需要复核。**Word、Excel、PowerPoint 文件可通过相应 Skill 处理，但不能在右侧文件预览器直接渲染，请用对应应用打开成品检查。费用是本地估算，不是服务商账单；截图中的缓存命中率不是性能承诺。
- **保护凭据。**API Token 只填入设置，不要放进对话、Issue 或公开截图；只安装可信来源的 Skill 和 MCP 服务。

## 从源码运行

界面使用 Electron、React 和 TypeScript；Rust Core 负责 Agent、工具执行、权限与本地存储。

需要 Node.js 22、pnpm 11.10.0、Rust stable（含 `rustfmt`、`clippy`），以及当前平台的原生编译工具链。

```bash
git clone https://github.com/Tiga001/Captain_Who.git
cd Captain_Who
pnpm install --frozen-lockfile
pnpm dev
```

首次启动会准备受管组件并编译 Rust。开发启动同样需要账户、软件许可和模型配置。安装包须在目标操作系统构建；存在 Windows/Linux 构建命令不代表已正式支持这些平台。

常用检查：`pnpm check` 运行常规检查与测试；`pnpm test:automation-core-e2e` 单独检查定时任务与 Core Server 的端到端行为。

[开发环境](docs/development/getting-started.md) · [系统架构](docs/architecture/overview.md) · [测试体系](docs/development/testing.md) · [构建与发布](docs/development/build-and-release.md)

## 文档与反馈

- [使用文档](public-docs/README.md)：安装、日常使用、能力与安全说明。
- [开发文档](docs/README.md)：架构、子系统、测试和维护约定。
- [GitHub Issues](https://github.com/Tiga001/Captain_Who/issues)：报告问题时，请附系统、应用版本和复现步骤，并移除密钥与私人内容。

欢迎通过 Issue 或 Pull Request 改进功能、修复问题和完善文档。提交代码前请运行与改动相关的检查和测试。

## 许可证

Captain Who 的自研代码以 [Apache License 2.0](LICENSE) 发布。第三方软件和资源保留各自许可证，详见[第三方软件声明](THIRD_PARTY_NOTICES.txt)与[语法文件第三方声明](THIRD_PARTY_GRAMMAR_NOTICES.txt)。
