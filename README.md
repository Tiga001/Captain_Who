# Captain Who

**让 AI 在你的项目里读资料、做文档、改代码，并把执行过程留给你检查。**

Captain Who 是一款本地优先的桌面 AI 工作助手。选择本地文件夹，接入你自己的模型 API，用自然语言说明目标；Agent 可以读取文件、搜索资料、调用工具、运行命令，也可以把独立任务交给多个子 Agent 并行处理。

文档、表格、演示文稿和代码改动都可以成为实际输出。你可以查看工具活动、核对文件差异，并按任务选择权限范围。

[快速开始](#快速开始) · [看看工作界面](#看看工作界面) · [从源码运行](#从源码运行) · [文档](public-docs/README.md) · [Releases](https://github.com/Tiga001/Captain_Who/releases) · [English](README.en.md)

![Captain Who in dark mode with a presentation plan and four active research sub-agents.](assets/screenshots/parallel-research.png)

*Track a presentation task while four research sub-agents work in parallel.*

> **开始前请确认：**当前正式安装包面向 macOS 12+、Apple Silicon。启动新的 Agent 任务需要 Captain Who 账户、可用的软件许可，以及你自行配置的模型 API。项目采用 Apache-2.0 开源许可证；模型调用额度不包含在内。

## 你可以用它做什么

| 你手上的工作 | Captain Who 可以参与的步骤 | 可检查的结果 |
| --- | --- | --- |
| 把零散资料整理成文档 | 读取项目文件或附件，使用文档 Skill 组织内容、生成文件并渲染检查 | Word 文档、PDF，以及引用和待确认事项 |
| 处理一份数据表 | 读取 XLSX、CSV 或 TSV，整理数据、补充公式、生成汇总 | 可继续编辑的工作簿；重要公式仍需复核 |
| 准备一次演示 | 拆分资料检索，整理大纲，使用演示文稿 Skill 生成并检查版式 | PPTX 文件和逐页内容 |
| 接手或修改一个代码项目 | 搜索代码、理解目录，按授权运行测试，通过 FileChange 提交修改 | 文件差异、命令结果和测试结论 |
| 搜索资料或操作网页 | 使用 Tavily 搜索和读取网页，或操作应用内受管浏览器 | 来源链接、页面信息、下载文件 |
| 定期检查同一件事 | 在“已安排”中设置周期任务，保留每次运行记录 | 检查报告、执行历史和需处理的审批 |

以上概述当前仓库记录的能力；开发分支可能领先于安装包。实际结果取决于模型、权限、运行组件和外部服务，生成内容需要人工复核。

## 看看工作界面

### 项目、文件差异和终端放在一起

围绕本地文件夹工作，在同一窗口查看文件、Git 差异与终端。Agent 的命令活动和文件变更也会留在对话时间线中，方便追踪。

![Light-mode workspace with a Python code diff and an integrated terminal.](assets/screenshots/code-review-terminal.png)

*Review code changes beside the conversation and an integrated workspace terminal.*

### 用 Skill 复用做事方法

内置文档、表格、演示文稿、PDF 等 Skill，也支持从公开 GitHub 仓库或本地文件夹安装。项目可以在 `.agents/skills/` 中保留自己的任务说明、模板和资源。

![Skills settings with GitHub and local-folder installation options.](assets/screenshots/install-skills.png)

*Install skills from GitHub or a local folder and manage built-in capabilities.*

### 按任务选择权限

提供默认、完全和自定义三种模式。可以控制文件范围、命令和变更审批，先从只读理解开始，再开放完成任务所需的能力。

![General settings with workspace access and approval controls.](assets/screenshots/permissions.png)

*Configure workspace access and approval rules for file edits, commands, and built-in tools.*

[查看完整截图画廊与英文说明](GALLERY.md)。截图中的配置仅用于展示，具体界面以安装版本为准；首次使用建议保留默认权限。

## 快速开始

### 1. 配置自己的模型

登录后，打开 **设置 → 配置**：

1. 填写模型服务的 **API URL** 和 **API Token**。
2. 进入 **管理模型**，添加服务商实际接受的 **模型 ID**，填写显示名称与上下文窗口。
3. 选择匹配的 Provider 配置，再在 **可用模型**中启用它。
4. 返回应用，新建对话，选择该模型，先发一个简短问题确认连接。

新安装的模型列表为空，需要手动添加并启用。当前支持 OpenAI-compatible、Anthropic-compatible，以及 DeepSeek、Moonshot 的专用配置。具体兼容要求见[连接模型 Provider](public-docs/integrations/model-provider-integration.md)。

**API Token 只应填入设置，不要粘贴到聊天、Issue 或截图中。**

### 2. 选一个文件夹，完成第一份文档

不用准备素材，先用下面这组虚构会议记录练习：

1. 在本机新建一个空文件夹，用作练习项目。
2. 在 Captain Who 点击 **新对话 → 项目选择器 → 新建项目**，选择这个文件夹。
3. 选择已配置的模型，保留 **默认权限**。
4. 点击输入框 **“+” → 技能**，选择内置 **文档（Documents）** Skill。
5. 复制下面的任务。出现审批时，核对目标文件、命令或变更内容后再决定是否批准。

```text
请使用文档 Skill，把下面的虚构会议记录整理成一份会议纪要。

项目：示例团队的季度分享会。
已确定：采用线上形式，内容包括产品演示和问题讨论。
行动项：小林整理演示提纲，小陈收集问题；截止日期尚未确定。
待确认：活动日期、时长和主持人。

分别列出已确定事项、行动项和待确认问题。
不要补写原文没有的负责人、日期或结论。
保存为 output/meeting-summary.docx，不要覆盖现有文件。
生成后渲染检查标题、分页和表格，并告诉我保存在哪里。
```

**完成后检查三件事：**输出文件是否实际存在，内容是否忠于原文，排版是否可用。DOCX、XLSX 和 PPTX 可以交给 Agent 处理，但不能在右侧“文件”预览器中直接渲染；请用对应应用打开成品复核。

不想先处理文件，也可以选择“不使用项目”，用普通对话确认模型连接。[第一个任务](public-docs/user/getting-started/first-task.md)和[第一个项目](public-docs/user/getting-started/first-project.md)提供更细的操作说明。

### 3. 逐步扩展工作方式

- **联网搜索与图片：**按需配置 Tavily 或图片生成服务的 API。它们不包含在聊天模型配置中。
- **浏览器：**开启应用内浏览器自动化，说明目标站点与允许动作。
- **Skills 与 MCP：**复用任务方法，或连接你信任的本机 stdio MCP Server。
- **多 Agent：**让独立的检索、实现或审查任务并行推进。
- **已安排：**为重复工作设置周期任务，查看执行历史与通知。

具体设置见[能力指南](public-docs/user/capabilities/README.md)。

## 本地优先，数据去向清楚

项目、对话、设置和执行记录主要保存在本机；添加项目或登录账户不会自动把本地工作内容同步到 Captain Who 的账户服务。

使用网络能力时，相关输入会发送给对应服务：模型接收任务上下文和必要的文件/工具内容，Tavily 接收搜索查询或目标网址，图片服务、目标网站和 MCP 接收完成操作所需的数据。账户登录与许可校验也需要连接账户服务。

模型等 API 密钥与普通配置分开保存。处理敏感资料前，请确认所选服务的数据政策；不要公开整个应用数据目录或历史数据库备份。详见[数据与权限](public-docs/security/data-and-permissions.md)。

## 使用时的几个边界

- **先用默认权限。**完全权限可以扩大文件访问并自动批准部分命令和变更；这些应用层策略不是操作系统沙箱，默认模式也不是只读模式。
- **检查实际结果。**停止任务不会撤销已经完成的写入或外部请求；文档、代码和表格结论仍需复核。
- **定时任务需要应用运行。**退出应用或关机后不会由系统后台唤醒任务。
- **能力取决于环境。**模型需支持相应输入和工具调用；浏览器自动化只控制应用内页面；Office 处理依赖对应 Skill。

<details>
<summary>文件格式、平台与其他限制</summary>

- 当前正式安装包面向 macOS 12+ Apple Silicon；Windows、Intel Mac 和 Linux 尚无正式支持的安装包。
- DOCX、XLSX 和 PPTX 可以通过对应 Skill 处理，但不能在右侧“文件”预览器直接渲染；请用原生应用打开成品。
- 旧版 `.doc` 的文本读取仅支持 macOS；`.ppt`、`.xls` 需要先转换。
- 扫描 PDF 不能保证直接提取文字，图片理解需要模型实际支持图像输入。
- 当前没有跨设备的项目、对话或模型密钥同步服务。
- 不同 API 网关的兼容性与第三方服务的费用由实际配置决定。

更多说明：[权限与审批](public-docs/user/everyday-use/permissions-and-approvals.md) · [Office 与 Artifact](public-docs/user/capabilities/artifacts-and-office.md) · [已知问题](public-docs/support/known-issues.md)

</details>

## 从源码运行

界面使用 Electron、React 和 TypeScript；Rust Core 负责 Agent、工具执行、权限与 SQLite 存储。这个仓库是实际开发仓库，源码版本与已发布安装包可能不同。

需要 Node.js **22**、pnpm **11.10.0**、Rust **stable**（含 `rustfmt`、`clippy`），以及当前平台的原生编译工具链。

```bash
git clone https://github.com/Tiga001/Captain_Who.git
cd Captain_Who
pnpm install --frozen-lockfile
pnpm dev
```

首次启动会准备受管组件并编译 Rust。具体步骤见[开发环境](docs/development/getting-started.md)。

<details>
<summary>开发检查、构建与架构</summary>

| 命令 | 用途 |
| --- | --- |
| `pnpm check` | 格式、文档、lint、类型、Clippy 和常规测试 |
| `pnpm test:unit` / `pnpm test:rust` | Node / Rust 测试 |
| `pnpm build` / `pnpm build:core` | Electron 输出 / release Core Server |
| `pnpm build:unpack` | 当前平台的未封装应用，供本地诊断 |

正式安装包必须在目标操作系统上构建。`pnpm build:mac` 还要求更新源配置和真实 Developer ID 签名；构建成功不代表完成 Apple 公证。Windows/Linux 构建入口不等于正式支持。开发启动同样需要满足当前账户、许可和模型配置要求。

主要目录：`src/main/`（Electron 主进程）、`src/renderer/`（React）、`crates/core/`（Agent、权限与存储）、`crates/core-server/`（Rust 服务）、`crates/mcp-client/`（MCP）和 `packages/`（协议、Host API、运行时）。Electron 与 Rust 通过逐行 JSON-RPC 通信。

[构建与发布](docs/development/build-and-release.md) · [测试体系](docs/development/testing.md) · [系统架构](docs/architecture/overview.md) · [仓库结构](docs/development/repository-layout.md)

</details>

## 文档与反馈

- [用户指南](public-docs/user/README.md)：首次配置与日常使用
- [能力指南](public-docs/user/capabilities/README.md)：Skill、MCP、多 Agent、浏览器与定时任务
- [Office 与 Artifact](public-docs/user/capabilities/artifacts-and-office.md)：文档、表格、演示、PDF 与图片
- [扩展与集成](public-docs/integrations/README.md)：模型接入、Skill 开发与 MCP
- [自助排查](public-docs/support/README.md)：常见故障、诊断与已知问题
- [开发文档](docs/README.md)：实现细节与贡献前的阅读入口

发现问题，可以在 [GitHub Issues](https://github.com/Tiga001/Captain_Who/issues) 描述系统与芯片、应用版本、复现步骤、预期结果和实际结果。截图与日志请先移除 Token、邮箱、私有路径和项目内容。

欢迎通过 Issue 或 Pull Request 改进文档、修复问题和完善功能。提交代码前请阅读开发文档，并运行与改动相关的检查和测试。

## 许可证

Captain Who 的自研代码以 [Apache License 2.0](LICENSE) 发布。第三方软件和资源保留各自许可证，详见[第三方软件声明](THIRD_PARTY_NOTICES.txt)与[语法文件第三方声明](THIRD_PARTY_GRAMMAR_NOTICES.txt)。
