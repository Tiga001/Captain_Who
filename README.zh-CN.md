<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="resources/brand-mark-dark.png">
    <img src="resources/brand-mark-light.png" width="88" alt="Captain Who 帆船标志">
  </picture>
</p>

<h1 align="center">Captain Who</h1>

<p align="center"><strong>一款开源的桌面 Agent 工作台。</strong></p>
<p align="center">接入自己的模型 API，查资料、写文档、改代码。</p>

<p align="center">
  <a href="https://github.com/Tiga001/Captain_Who/stargazers"><img src="https://img.shields.io/github/stars/Tiga001/Captain_Who?style=flat-square&amp;color=1f6feb" alt="GitHub Stars"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/Tiga001/Captain_Who?style=flat-square&amp;color=1f6feb" alt="许可证：Apache-2.0"></a>
  <a href="public-docs/releases/supported-platforms.md"><img src="https://img.shields.io/badge/macOS-12%2B%20%C2%B7%20Apple%20Silicon-1f6feb?style=flat-square" alt="正式安装包：macOS 12+、Apple Silicon"></a>
</p>

<p align="center"><a href="README.md">English</a> · 简体中文</p>
<p align="center">
  <a href="https://captainwhoagent.com/">官网与下载</a> ·
  <a href="#快速开始">快速开始</a> ·
  <a href="public-docs/README.md">使用文档</a> ·
  <a href="GALLERY.md">截图画廊</a> ·
  <a href="https://github.com/Tiga001/Captain_Who/issues">问题反馈</a>
</p>

Captain Who 将对话、本地文件、终端和浏览器放在同一个工作界面里。你可以与一个 Agent 工作，让它并行派出子智能体，也可以通过 **组织（Organizations）** 把多段独立对话组成团队，用内置邮箱交流协作。

> **版本与平台：** 正式安装包支持 macOS 12 及以上的 Apple Silicon Mac。本 README 也介绍当前源码中的组织和内置本地登录功能，这些能力可能领先于官网安装包，下载前请查看[发行信息](public-docs/releases/README.md)。模型 API 需要自行配置，软件不包含模型调用额度。

![四个子智能体正在为浙江大学介绍 PPT 分别检索资料。](assets/screenshots/parallel-research.png)

_将一份大学介绍 PPT 的资料检索拆成四项并行任务。图中展示的是执行过程，不是最终幻灯片成品。_

## 先试一个任务

准备一个放有非敏感笔记或参考资料的文件夹，连接模型后发送：

```text
请阅读这个项目里的资料，整理一份摘要，保存为 summary.md。
列出主要结论、对应来源和仍需确认的问题，不要修改原始文件。
```

核对执行中的审批请求，完成后在对话旁打开 `summary.md`，检查结论是否有原文依据。这个例子不需要配置联网搜索，也不需要先搭建多智能体团队。

## 快速开始

1. **安装应用。** 从 [captainwhoagent.com](https://captainwhoagent.com/) 下载。Windows、Intel Mac 和 Linux 目前没有正式公开安装包。想尝试当前开发代码，可以[从源码运行](#从源码运行)。
2. **登录。** 源码构建中，选择“账号密码”，账号和密码均填写 `captainwho`。这是公开的本地登录入口，无需云端注册或在线许可校验。下载安装包的用户，请按所用版本提供的账户流程操作。详见[账户与软件许可](public-docs/user/getting-started/account-and-license.md)。
3. **连接模型。** 进入 **设置 → 配置**，填写 API URL 和 Token；在 **管理模型** 中添加服务商实际接受的模型 ID，并启用。新安装没有预置模型。
4. **选择项目并发送任务。** 打开 **新对话 → 项目选择器 → 新建项目**，选择本地文件夹，再选择模型并保留默认权限。核对审批和最终生成的文件。

模型接入支持 **OpenAI Chat Completions-compatible**、**Anthropic Messages-compatible** 接口，以及 DeepSeek、Moonshot 配置。实际兼容程度取决于服务商实现。联网搜索与图片生成需要另行配置对应服务。

[安装指南](public-docs/user/getting-started/installation.md) · [模型配置](public-docs/integrations/model-provider-integration.md) · [第一个任务](public-docs/user/getting-started/first-task.md)

## 组织：让独立对话组成团队

**当前开发源码已提供，请核对所装版本是否包含此功能。**

不必预先画好 Agent 之间的执行路线。给每位成员分配职责和邮箱，让它们根据任务互相求助、交换发现、安排工作。

- **独立对话。** 每位成员都有自己的对话、模型、职责和工具权限，你可以随时打开并直接与它交流。
- **内置邮箱。** 成员在 Captain Who 内部收发消息，不需要外部电子邮箱，部门划分也不限制成员互相通信。
- **可以调整的团队。** 拥有管理权限的成员可以在授权范围内创建部门，增加、修改或移除低职级成员。人员管理权限不等于无限的工具使用权限。

可以先建立一个小型调研团队：在模板中添加 **研究员** 和 **审阅员**，配置模型后激活组织。给研究员提供两篇公开资料，再发送：

```text
比较这两篇资料中的方案，把关键结论和来源通过组织邮件发给审阅员，
请他核对证据。根据反馈给我一版简短结论，并列出仍未解决的问题。
不要对外发送消息，也不要修改文件。
```

![左侧是独立研究对话，中间说明组织邮件工具，右侧是部门与成员看板。](assets/screenshots/organization-research.png)

_一个规模更大的研究组织，成员分别负责方法提取、适用性评估、数学审查、数值验证和归档。_

**组织与子智能体的区别：** 子智能体承接一段父对话里派出的任务；组织让多段独立对话组成团队。组织成员仍可派遣子智能体。额外的沟通可能增加 Token 消耗，小任务不一定需要组建团队。

[创建一个组织](public-docs/user/tutorials/create-an-organization.md) · [组织行为与权限](public-docs/user/capabilities/organizations.md)

## 工作台中的其他能力

| 能力                                                                                                                          | 可以做什么                                                                              |
| ----------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| [文件、代码与终端](public-docs/user/capabilities/tools.md)                                                                    | 读写项目文件、运行命令、查看 Git 差异，在对话旁核对改动。                               |
| [浏览器](public-docs/user/capabilities/browser-automation.md)与[人机交互](public-docs/user/capabilities/human-interaction.md) | 让 Agent 操作网页，在需要判断或登录接管时向你请求协助。                                 |
| [Skills](public-docs/user/capabilities/skills.md) 与 [MCP](public-docs/user/capabilities/mcp.md)                              | 安装可复用的任务方法、连接外部工具，配合相应 Skill 处理文档、表格、幻灯片、PDF 和图片。 |
| [子智能体](public-docs/user/capabilities/multi-agent.md)                                                                      | 并行派遣调研、撰写或审阅任务，复用专家模板。                                            |
| [定时任务](public-docs/user/capabilities/automations.md)                                                                      | 安排周期工作、查看执行记录；定时执行需要应用保持运行。                                  |
| [上下文](public-docs/user/learn/context-management.md)与[用量](public-docs/user/reference/settings.md#用量与费用估算)         | 管理长对话，查看模型用量、缓存 Token 和费用估算。                                       |

详细操作见对应能力文档。模型、权限和可选服务均可自行配置。

![Python 代码差异与对话并排，底部打开项目终端。](assets/screenshots/code-review-terminal.png)

_在任务界面中核对文件改动。[查看更多截图 →](GALLERY.md)_

## 使用前需要知道

- **本地优先不等于离线。** 模型服务商和已启用的搜索、图片、浏览器或 MCP 服务会收到任务所需的数据。组织使用多个模型服务商时，公共背景与邮件可能发送给不止一家服务商。详见[数据与权限](public-docs/security/data-and-permissions.md)。
- **权限不是操作系统沙箱。** 默认模式不是只读模式。建议从默认权限开始，核对文件修改、命令执行和外部操作请求；截图中的完全权限不是推荐设置。停止任务不会撤销已经完成的操作。
- **结果需要检查。** 核对来源和文件改动。Skill 可以创建或处理 Office 文件，但侧栏文件预览器不能直接渲染 Word、Excel 或 PowerPoint，请用相应应用打开成品。详见[文档处理能力](public-docs/user/capabilities/artifacts-and-office.md)。
- **关注费用与凭据。** 费用是估算，不是服务商账单，截图中的缓存命中率也不是性能承诺。API Key 只填入设置，不要放进聊天、Issue 或截图；只安装可信来源的 Skill 和 MCP 服务。

## 从源码运行

桌面界面使用 Electron、React 和 TypeScript；Rust Core 负责 Agent、工具、权限和本地存储。

需要 **Node.js 22**、**pnpm 11.10.0**、通过 **rustup** 安装的 Rust，以及当前平台的原生编译工具链。仓库在 [rust-toolchain.toml](rust-toolchain.toml) 中锁定 **Rust 1.99.0**，并指定 `rustfmt` 和 `clippy`。

```bash
git clone https://github.com/Tiga001/Captain_Who.git
cd Captain_Who
pnpm install --frozen-lockfile
pnpm dev
```

首次启动会准备受管组件并编译 Rust。启动后按[快速开始](#快速开始)完成本地登录和模型配置。存在其他操作系统的构建命令，不代表这些平台已经正式支持。

参与开发时，`pnpm check` 运行常规检查与测试；`pnpm test:automation-core-e2e` 单独检查定时任务与 Core Server 的端到端行为。

[开发环境](docs/development/getting-started.md) · [系统架构](docs/architecture/overview.md) · [测试体系](docs/development/testing.md) · [构建与发布](docs/development/build-and-release.md)

## 文档与参与贡献

- [使用文档](public-docs/README.md)：安装、日常任务、能力与安全说明，中文为主。
- [开发文档](docs/README.md)：架构、子系统、测试与维护约定，包含中英文内容。
- [GitHub Issues](https://github.com/Tiga001/Captain_Who/issues)：反馈 Bug 和功能需求。请附系统、应用版本和复现步骤，并移除密钥与私人内容。

Captain Who 是一个独立开发项目。欢迎贡献代码、文档和翻译，也欢迎提供可复现的问题报告。提交 Pull Request 前请运行与改动相关的检查。

## 许可证

Captain Who 的自研代码以 [Apache License 2.0](LICENSE) 发布。第三方软件和资源保留各自许可证，详见[第三方软件声明](THIRD_PARTY_NOTICES.txt)与[语法文件第三方声明](THIRD_PARTY_GRAMMAR_NOTICES.txt)。
