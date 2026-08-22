---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-23
---

# 故障排查

先保留首次失败的完整退出码和无敏感信息日志。不要以删除数据目录、关闭校验或反复重试未知副作用
作为第一步。

## 安装或原生模块失败

确认 Node 为 22、pnpm 与 `package.json#packageManager` 一致，并在目标操作系统上安装原生编译工具链。
然后重新执行：

```bash
pnpm install --frozen-lockfile
```

`node-pty` 需要本机原生构建。跨平台复制 `node_modules`、`.cache` 或打包输出不是受支持的修复方式。

## `pnpm dev` 在组件准备阶段失败

按失败组件单独验证：

```bash
pnpm verify:officecli
pnpm verify:office-renderer
pnpm verify:word-pdf-renderer
pnpm verify:artifact-runtime
```

组件准备脚本校验冻结版本、文件和 receipt。不要手工修改 `.cache/*/current` 来绕过失败。组件的来源、
平台支持与打包位置见[运行时组件](runtime-components.md)。

## Core Server 未启动或意外退出

1. 运行 `pnpm build:core`，确认 release 二进制能够构建并通过校验。
2. 开发模式检查 Cargo/Rust 错误；打包模式检查 `process.resourcesPath` 中对应平台的 `core-server`。
3. 确认没有把 Windows、macOS 或 Linux 的二进制混入其他平台产物。
4. 检查 JSON-RPC framing 错误时，不要直接打印原始敏感 payload。

开发模式的启动命令和打包模式的资源路径均由 Main 决定，不提供用户可配置的 sidecar 路径覆盖。
`MYCOPILOT_CORE_SERVER_PATH` 只出现在仓库自有 Office 渲染测试中，不能用于替换应用 sidecar。启动、
重启和停机语义见[Electron Host](../architecture/electron-host.md)与[Core Server](../architecture/core-server.md)。

## `development_storage_schema_reset_required`

当前开发策略只接受规范 schema 及 catalog fingerprint，不对旧库原地迁移。完全退出 MyCopilot 后执行：

```bash
pnpm storage:reset-dev
pnpm storage:reset-dev -- --confirm-reset
```

第一条是非破坏性预检，第二条才会备份并重建。应用或 Core Server 仍持锁时命令会拒绝执行。不要删除原库或
手工修改 `PRAGMA user_version`；详见[存储与数据生命周期](../architecture/storage-and-data-lifecycle.md)。

## 页面一直停留在启动状态

App startup gate 会等待项目、模型和 Host 状态。先定位具体 stage 的错误，不要让 Renderer 用默认值伪装
成成功水合。常见原因包括 Core Server 未就绪、协议 parser 拒绝响应、schema reset required 或模型配置无效。

## MCP Server 无法连接

- 确认用户配置的 MCP Server 仍是本地 stdio，而不是 HTTP URL。
- 保存配置不等于授权启动；可执行文件、argv、cwd 或绑定代码文件变化后必须重新授权。
- 检查 Server 是否 enabled，以及 Catalog 是否完整、无重名和 schema 超限。
- `OutcomeUnknown` 表示调用可能已经送出，不能自动重试。
- 不要用 `npx`、真实用户 Server 或继承环境作为测试 fixture。

外部 MCP 与内部 HostBridge/Playwright 是不同路径，详见[MCP](../subsystems/mcp.md)。

## 浏览器自动化失败

先区分手动 Browser Surface、能力激活、敏感目标授权和 Managed Playwright dispatch 哪一层失败。确认：

- 当前任务已获得 `browser_automation` 能力授权；
- 目标 origin 与授权绑定一致；
- 需要页面的工具已绑定有效 surface/page；
- 下载、上传、截图和文件访问经过 Host broker；
- fixed catalog、schema digest 与受管 Playwright 版本一致。

运行发布专项检查见[测试体系](testing.md)。当前 packaged startup 验证仍不等于完整的打包 Agent→MCP
链路通过，不能扩大测试结论。

## Office、PDF 或 Artifact 失败

先运行相应 `verify:*` 命令。Builder/Editor 脚本必须经过 Host 预检并在受管运行时中执行；输出必须通过
schema、渲染或 publication gate。不要把 QA PDF 混入正式输出，不要直接向私有 Artifact object 路径
授予 Renderer/模型权限。

详见[Office 与 Artifact](../subsystems/office-and-artifacts.md)。

## Terminal 或 Command Session 卡住

内置 Terminal 与 Agent `run_command`/`command_session` 是不同生命周期。对于 Agent 命令：

- `running` 不是成功；构建和测试应调用一次 `command_session wait` 获得终态；
- GUI 或长期服务通常不等待自然退出；
- `outcome_unknown` 是终态但结果未知，不能宣称成功或继续运行；
- 任意 stdin 目前不属于 `command_session` 契约。

Terminal 输出问题应检查 sequence/ACK/背压和 Renderer/WebContents 所有权，见[终端](../subsystems/terminal.md)。

## 打包或 macOS 签名失败

安装包必须在目标系统原生构建。macOS `build:mac` 强制签名、hardened runtime 和严格验签；当前未配置
notarization。不要用 `--config.mac.identity=null` 的 unpack 结果冒充可发布安装包。完整步骤见
[构建与发布](build-and-release.md)。

## 仍无法定位

最小化为仓库自有、无网络、无真实凭据的 fixture；记录平台、命令、commit、首个失败和可重复步骤。
如果问题涉及崩溃恢复或不确定副作用，按[恢复 Runbook](../operations/recovery-runbook.md)处理。
