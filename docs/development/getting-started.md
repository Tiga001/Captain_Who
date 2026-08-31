---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-31
---

# 开发环境与启动

## 前置要求

- Node.js 22，版本约束见 [`.node-version`](../../.node-version) 和 [`package.json`](../../package.json)。
- pnpm 11.10.0；应使用仓库声明的 `packageManager` 版本。
- Rust stable，并安装 `rustfmt`、`clippy`。
- 当前平台的原生编译工具链。`node-pty` 与 Core Server binary 都包含原生构建步骤。
- Browser tests 会通过 `pnpm test:browser` 准备受管的锁定 Chromium；`pnpm test:web:install` 仅安装普通 Playwright Chromium，不能替代受管组件真源。

安装依赖：

```bash
pnpm install --frozen-lockfile
```

不要绕过 lockfile 更新单个运行时依赖。Office、Artifact 和 Managed Playwright 组件均有独立的冻结版本与校验
receipt，详见[运行时组件](runtime-components.md)。

## 启动开发环境

```bash
pnpm dev
```

该命令先准备 OfficeCLI、Office Renderer、当前平台支持的 Word/PDF Renderer、Artifact Runtime 和开发
图标，再启动 Electron/Vite。开发模式下 Main 启动 Core Server；首次构建通常比后续启动慢。

应用首次进入后，在“设置 → 配置”中创建 Provider/Model 配置，并按需设置 Tavily、图片生成、MCP 和
Skills。配置所有权和敏感值规则见[设置与配置](settings-and-configuration.md)。

## 常用命令

| 命令                     | 用途                                                 |
| ------------------------ | ---------------------------------------------------- |
| `pnpm format`            | 格式化 TypeScript、CSS、Markdown 与 Rust             |
| `pnpm format:check`      | 只检查格式                                           |
| `pnpm check:docs`        | 检查文档元数据、链接、路径、命令与版本真源           |
| `pnpm check:test-layout` | 检查 Vitest 唯一归属与 Rust ignored-test registry    |
| `pnpm lint`              | ESLint                                               |
| `pnpm typecheck`         | Main/Preload/Renderer TypeScript 检查                |
| `pnpm lint:rust`         | Rust workspace 严格 Clippy                           |
| `pnpm test:unit`         | Node Vitest unit project                             |
| `pnpm test:browser`      | locked Chromium browser project                      |
| `pnpm test:electron`     | 真实 Electron fixture 与 Managed Playwright E2E      |
| `pnpm test:web`          | unit、browser 与 Electron 聚合入口                   |
| `pnpm test:rust`         | Rust workspace 测试                                  |
| `pnpm test`              | 脚本、Renderer/Browser/Electron 与 Rust 完整常规测试 |
| `pnpm check`             | 格式、文档、lint、类型、Clippy 与测试总门禁          |
| `pnpm build`             | TypeScript 检查并构建 Electron 输出                  |
| `pnpm build:core`        | 构建并校验 release Core Server binary                |
| `pnpm build:unpack`      | 构建当前平台的未封装应用                             |

`pnpm check` 当前不自动包含 Multi-Agent release gate、Playwright release gate 或平台签名验证。发布前还
需执行[测试体系](testing.md)和[构建与发布](build-and-release.md)指定的门禁。

`.github/workflows/tests.yml` 会在 pull request、`main` push 和手动触发时分开执行 Linux 静态/脚本/Node/browser/Rust、macOS Electron、Automation 真实 Core Server 与 Multi-Agent release gate；它不是发布打包或签名流水线。

## 数据与本地诊断

Electron 使用 `app.getPath('userData')` 作为权威数据根，并显式传给 Core Server。不要在业务代码中猜测
平台路径。独立运行 `core-server` 时可用 `MYCOPILOT_STORAGE_DB` 指定测试数据库；正式 Electron 启动
会以 Host 传入的数据根为准。

开发库 schema 不兼容时，完全退出应用后先预检：

```bash
pnpm storage:reset-dev
```

确认摘要后再执行：

```bash
pnpm storage:reset-dev -- --confirm-reset
```

该流程会创建校验过的备份再发布 fresh canonical database。完整保留矩阵和恢复步骤见
[存储与数据生命周期](../architecture/storage-and-data-lifecycle.md)与[恢复 Runbook](../operations/recovery-runbook.md)。

## 开发约束

- Renderer 不直接访问 Node、文件系统、数据库或 Core Server；只能使用类型化 Host API。
- 跨进程 DTO 的变更必须同步 TypeScript/Rust 契约和 fixture。
- 新工具、审批、数据表或运行时组件必须同步相应文档和测试。
- 不把 Token 放入命令行、MCP argv、日志、fixture、截图或文档。
- 不使用真实付费模型、用户 MCP Server 或用户文件作为确定性测试 fixture。

启动失败时先查看[故障排查](troubleshooting.md)，不要直接删除数据目录或绕过 receipt/签名校验。
