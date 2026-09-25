# Captain Who

简体中文 | [English](README.en.md)

Captain Who 是一个本地优先的桌面 AI 工作助手。界面由 Electron、React 和 TypeScript 构建，Agent、工具执行与本地存储由 Rust Core 负责。

## 下载与源码

想直接使用，请到 [Captain Who 官网](https://captainwhoagent.com/) 下载 macOS 安装包。当前正式版支持 macOS 12 及更高版本的 Apple Silicon Mac；安装包、校验信息和升级说明见[版本与发布](public-docs/releases/README.md)。

目前没有 Windows 安装包。项目由个人开发，现阶段没有足够时间完成 Windows 版本，也没有确定的发布时间。

本 [GitHub 仓库](https://github.com/Tiga001/Captain_Who)是 Captain Who 的实际开发仓库。项目已按 [Apache License 2.0](LICENSE) 开源，仓库会持续更新。开发版本可能领先于官网已发布的安装包；直接使用软件时请以官网和[发行说明](public-docs/releases/release-notes/README.md)为准。

目前已实现：

- 本地项目、对话、草稿、归档与全文搜索
- Provider Profile、OpenAI-compatible、Anthropic-compatible 与 DeepSeek 模型接入和安全切换
- 文件、图片、PDF、Word、演示文稿和表格附件读取
- 工作区搜索、统一 FileChange 创建/更新/删除、Git diff、受管命令与持久 Command Session
- 可审批的文件/命令操作，以及默认、完全、自定义三种权限模式
- Tavily 联网搜索与网页读取
- 内置终端、手动浏览器、Managed Playwright、持久浏览/下载历史、下载中心和浏览器设置
- 用户配置的 stdio MCP Server、内部 HostBridge Capability、bundled/installed/workspace Skill
- 多智能体协作、项目可分配 Agent 模板、树内附件/Artifact 共享、只读观察和崩溃恢复
- Scheduled Automation：按结构化计划启动 Agent Run、保留历史与 attention，并通过原生通知提醒
- 普通任务与 Automation 共用的持久系统通知、声音/内容预览和按状态通知策略
- Word、表格、演示文稿、PDF 与图片生成的受管 Artifact 工作流
- 跨轮 Agent 工具轨迹、上下文压缩、Exact Archive 与长期使用量提示
- 简体/繁体中文、英式/美式英语、日语、韩语、法语、意大利语和俄语界面

## 架构

```text
src/main/                 Electron 主进程、分域 IPC、浏览器/终端桥接
src/preload/              隔离的 Renderer Host API
src/renderer/             React 界面
packages/protocol/        TypeScript 跨进程数据类型
packages/host-api/        Renderer 可调用的 Host API 类型
crates/protocol-rs/       Rust JSON-RPC 协议
crates/core/              Agent、工具、权限与 SQLite 存储
crates/core-server/       Core Server 应用边界（application / transport / adapters）
crates/mcp-client/        MCP 协议、Catalog、连接和 stdio transport
packages/artifact-runtime-node/ 受管 Artifact Runtime 的 Node 入口
scripts/                  组件准备、测试、打包、签名和发布验证
public-docs/              面向用户、集成开发者和支持场景的公开文档
```

开发模式下，Electron 通过 Cargo 启动 `core-server`；生产包会把 release 二进制复制到 `process.resourcesPath`。Electron 与 Rust 之间使用逐行 JSON-RPC 通信。

完整的架构、子系统、开发、测试、发布和安全文档统一从
[开发文档索引](docs/README.md)进入。新成员建议先阅读[开发环境](docs/development/getting-started.md)、
[仓库结构](docs/development/repository-layout.md)和[系统架构总览](docs/architecture/overview.md)。
产品使用、集成、发布状态和自助排查从[公开文档索引](public-docs/README.md)进入。

## 许可证

Captain Who 的自研代码以 [Apache License 2.0](LICENSE) 发布。仓库与安装包中包含的第三方软件、
资源与语法文件继续适用其原有许可证；完整归属与声明见
[第三方软件声明](THIRD_PARTY_NOTICES.txt)和
[语法文件第三方声明](THIRD_PARTY_GRAMMAR_NOTICES.txt)。

## 环境要求

- Node.js 22（见 `.node-version`）
- pnpm 11.10.0（见 `package.json#packageManager`）
- Rust stable，包含 `rustfmt` 与 `clippy`
- 当前平台的原生编译工具链；`node-pty` 和 Rust sidecar 都需要本机编译

安装依赖：

```bash
pnpm install --frozen-lockfile
```

首次开发启动会编译 Rust，耗时会比后续启动长：

```bash
pnpm dev
```

应用启动后，在“设置 → 配置”中填写模型 API URL、Token、模型标识与可选的 Tavily API Key。仓库不再内置机构地址或占位搜索 Key。

## 常用命令

| 命令                            | 用途                                            |
| ------------------------------- | ----------------------------------------------- |
| `pnpm dev`                      | 启动 Electron 开发环境与 Core Server            |
| `pnpm format`                   | 格式化 TypeScript、CSS、文档与 Rust             |
| `pnpm check:docs`               | 检查文档元数据、链接、路径、命令与版本真源      |
| `pnpm check:public-docs`        | 检查公开文档结构、索引和边界                    |
| `pnpm check:test-layout`        | 检查测试文件归属和 ignored Rust 测试登记        |
| `pnpm lint`                     | 运行 ESLint                                     |
| `pnpm typecheck`                | 检查 Main、Preload 与 Renderer 类型             |
| `pnpm lint:rust`                | 对整个 Rust workspace 运行严格 Clippy           |
| `pnpm test:unit`                | 运行 Node Vitest unit project                   |
| `pnpm test:browser`             | 使用锁定 Chromium 运行 Vitest browser project   |
| `pnpm test:electron`            | 运行真实 Electron fixture 与 Managed Playwright |
| `pnpm test:web`                 | 聚合 unit、browser 与 Electron 测试             |
| `pnpm test:automation-core-e2e` | Automation Host API 与真实 Core Server 专项 E2E |
| `pnpm test:rust`                | 运行 Rust workspace 测试                        |
| `pnpm test`                     | 运行脚本、Web/Browser 与 Rust 常规测试          |
| `pnpm check`                    | 执行格式、文档、lint、类型、Clippy 和测试       |
| `pnpm build`                    | 类型检查并生成 Electron 的 `out/` 产物          |
| `pnpm build:core`               | 构建并校验 release Core Server binary           |
| `pnpm build:unpack`             | 生成当前平台的未封装应用，用于打包冒烟测试      |

## 打包

安装包必须在对应操作系统的原生 runner 上构建；脚本会拒绝从 macOS 直接生成 Windows/Linux 包，避免把错误格式的 Rust 二进制带入安装包。

```bash
# Windows
pnpm build:win

# macOS
pnpm build:mac

# Linux
pnpm build:linux
```

`electron-builder.yml` 只把 `out/`、运行时资源、生产依赖和当前平台的 `core-server` 放入应用，不会再把源码或 Cargo `target/` 缓存打进 ASAR。

macOS `build:mac` 已强制 Developer ID 签名、hardened runtime、受管原生组件与隐私门禁验签，DMG
容器也会签名。官网提供的 1.0.5 正式 DMG 已通过 Apple 公证并装订公证凭据，公开更新源也已启用；
应用会在有新版本时提供由用户发起的下载和安装。仓库的构建命令本身不执行 Apple 公证，`pnpm check`
也不包含全部专项发布门禁。自行构建的产物不能直接视为正式安装包，发布流程见
[构建与发布](docs/development/build-and-release.md)。

## 本地数据与隐私

Electron 应用以 `app.getPath('userData')` 返回的位置作为唯一权威数据根目录，并在启动
Core Server 时显式传入该目录。数据库位于数据根的 `storage.sqlite`，附件、已安装 Skill、
生成图片和未签名 macOS 开发环境凭据分别保存在同级的受管子目录中；启动时会清理无数据库引用的孤立
附件文件。具体路径由 Electron 按当前操作系统和应用身份解析，业务代码不再分别猜测
macOS、Windows 或 Linux 的目录。

直接运行独立 `core-server` 时仍可通过 `MYCOPILOT_STORAGE_DB` 指定数据库路径；该变量是
测试和独立诊断接口，Electron 启动的正式应用会使用 Host 传入的数据根覆盖它。

开发期需要重建 SQLite 基线时，先完全退出 Captain Who，再运行非破坏性预检：

```bash
pnpm storage:reset-dev
```

确认预检摘要后，显式执行重建：

```bash
pnpm storage:reset-dev -- --confirm-reset
```

命令通过 Electron 解析同一个权威数据根；应用或 Core Server 仍持有数据库时会拒绝执行。确认
重建会先在数据根的 `storage-backups/` 中创建权限受限、经过 SQLite 校验的时间戳备份，
再原子发布 fresh canonical database。模型与搜索配置、UI/Prompt 偏好、Skill 启用状态、
MCP Server 配置、通知设置、浏览器下载/链接偏好和有效的图片生成 Profile 会通过当前严格写入路径恢复；
对话、项目、Agent 模板、草稿、浏览/下载历史、通用通知事实、Automation 任务/Run/event/outbox、
Usage、审批、Continuation、Compaction、Fork 等状态不会恢复。
附件、已安装 Skill、
生成图片和凭据目录不会在重建事务中被删除或搬移；与已清理对话绑定的附件记录不会恢复，
其文件会在应用后续正常启动时按现有孤立附件策略清理。命令只输出路径和计数，不输出 Token
或配置值。

内置浏览器使用独立的持久会话，并在 Rust Core 中保存浏览历史、下载记录和打开偏好；“清除浏览数据”
可按类别和时间范围清理历史、Cookie/站点数据、缓存或下载记录。站点图标由 Main 通过受管会话获取，
缓存最多保留 256 项和 30 天。

请注意：

- 模型 Token、Tavily Key 与图片生成 API Key 都与普通配置分离；当前 v33 SQLite 只保存
  credential reference、配置状态和非秘密元数据。Renderer 只取得凭据状态和用户本次新输入的值，
  不会读回已有密钥或 reference。
- 具备稳定签名身份的发行构建使用操作系统凭据存储；未签名 macOS 开发构建使用数据根内
  目录权限 `0700`、文件权限 `0600` 的私有文件 backend。完整数据根仍应视为敏感数据。
- 当前 SQLite 备份不包含当前模型/搜索 secret，但旧 schema 的历史备份可能仍含明文凭据；
  只恢复 SQLite 不会恢复操作系统凭据。清除/删除不承诺对 SSD、系统备份或系统凭据后端安全擦除，
  怀疑泄露时应在 Provider 侧撤销或轮换密钥。
- 模型请求会发送到你配置的 API URL；启用联网搜索后，查询或目标 URL 会发送给 Tavily。
- 内置浏览器默认拒绝网页申请摄像头、麦克风、定位、通知等系统权限。
- “移除项目”会永久删除 Captain Who 中该项目的本地对话、消息与附件，但不会修改项目目录中的文件。
- 费用只是按模型设置中的每 1k token 单价计算的本地估算，不代表服务商账单，也不区分币种。

## 文档与 Artifact 支持

- Agent 附件读取支持 `.docx`、`.pptx`、`.xlsx`、`.csv`、`.tsv` 等格式；右侧栏文件预览的支持范围
  与附件读取不同，见[工作区文件](docs/subsystems/workspace-files.md)。
- 旧版 `.doc`：仅在 macOS 上通过系统 `textutil` 解析
- 旧版 `.ppt`、`.xls`：暂不支持，请先转换为 `.pptx`、`.xlsx` 或文本格式
- PDF 附件可提取文本；扫描件是否可读取取决于 PDF 是否包含文本层。复杂 PDF 处理由受管 PDF Skill
  和命令工作流提供。
- Word、表格和演示文稿创建/编辑通过受管 Builder、Editor、Renderer 和 Artifact 发布门禁完成，详见
  [Office 与 Artifact](docs/subsystems/office-and-artifacts.md)。
