# MyCopilot Next

MyCopilot 是一个本地优先的桌面 AI 工作助手。界面由 Electron、React 和 TypeScript 构建，Agent、工具执行与本地存储由 Rust 核心负责。

目前已实现：

- 本地项目、对话、草稿、归档与全文搜索
- OpenAI-compatible 与 Anthropic-compatible 模型接入
- 文件、图片、PDF、Word、演示文稿和表格附件读取
- 工作区搜索、补丁编辑、流式文件写入、Git diff 与命令执行
- 可审批的文件/命令操作，以及默认、完全、自定义三种权限模式
- Tavily 联网搜索与网页读取
- 内置终端、浏览器、用量统计和本地费用估算

## 架构

```text
src/main/                 Electron 主进程、IPC、浏览器/终端桥接
src/preload/              隔离的 renderer Host API
src/renderer/             React 界面
packages/protocol/        TypeScript 跨进程数据类型
packages/host-api/        Renderer 可调用的 Host API 类型
crates/protocol-rs/       Rust JSON-RPC 协议
crates/core/              Agent、工具、权限与 SQLite 存储
crates/core-server/       Electron 启动的 Rust sidecar
```

开发模式下，Electron 通过 Cargo 启动 `core-server`；生产包会把 release 二进制复制到 `process.resourcesPath`。Electron 与 Rust 之间使用逐行 JSON-RPC 通信。

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

| 命令                | 用途                                        |
| ------------------- | ------------------------------------------- |
| `pnpm dev`          | 启动 Electron 开发环境与 Rust core-server   |
| `pnpm format`       | 格式化 TypeScript、CSS、文档与 Rust         |
| `pnpm lint`         | 运行 ESLint                                 |
| `pnpm typecheck`    | 检查主进程、preload 与 renderer 类型        |
| `pnpm lint:rust`    | 对整个 Rust workspace 运行严格 Clippy       |
| `pnpm test`         | 运行 Rust workspace 测试                    |
| `pnpm check`        | 依次执行格式、lint、类型、Clippy 和测试检查 |
| `pnpm build`        | 类型检查并生成 Electron 的 `out/` 产物      |
| `pnpm build:core`   | 构建并校验 release core-server              |
| `pnpm build:unpack` | 生成当前平台的未封装应用，用于打包冒烟测试  |

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

当前项目没有配置代码签名、macOS notarization 或自动更新发布地址。对外分发前需要单独补齐这些发布环节。

## 本地数据与隐私

默认数据库位置：

- macOS：`~/Library/Application Support/mycopilot-next/storage.sqlite`
- Windows：`%APPDATA%/mycopilot-next/storage.sqlite`
- Linux：`$XDG_DATA_HOME/mycopilot-next/storage.sqlite`，未设置时使用 `~/.local/share/mycopilot-next/storage.sqlite`

设置 `MYCOPILOT_STORAGE_DB` 可以覆盖数据库路径。附件保存在数据库同级的 `attachments/` 目录；启动时会清理无数据库引用的孤立附件文件。

内置浏览器使用独立的持久会话；“清除浏览数据”会同时清理该会话和站点图标缓存。图标缓存最多保留 256 项和 30 天。

请注意：

- 模型 Token 与 Tavily Key 当前以明文保存在本机 SQLite 数据库中，不是系统钥匙串。
- 模型请求会发送到你配置的 API URL；启用联网搜索后，查询或目标 URL 会发送给 Tavily。
- 内置浏览器默认拒绝网页申请摄像头、麦克风、定位、通知等系统权限。
- “移除项目”会永久删除 MyCopilot 中该项目的本地对话、消息与附件，但不会修改项目目录中的文件。
- 费用只是按模型设置中的每 1k token 单价计算的本地估算，不代表服务商账单，也不区分币种。

## 文档格式支持

- `.docx`、`.pptx`、`.xlsx`、`.csv`、`.tsv`：跨平台解析
- 旧版 `.doc`：仅在 macOS 上通过系统 `textutil` 解析
- 旧版 `.ppt`、`.xls`：暂不支持，请先转换为 `.pptx`、`.xlsx` 或文本格式
- PDF：文本提取；扫描件是否可读取决于 PDF 是否包含文本层

## 仓库状态

项目当前为私有、`UNLICENSED`。不要在未补充许可证、凭据安全方案、签名与发布流程前直接公开发布。
