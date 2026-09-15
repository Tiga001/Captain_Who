---
status: current
audience: developers
owner: engineering
last_verified: 2026-09-16
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

当前开发策略只接受规范 schema 及 catalog fingerprint，不对旧库原地迁移。完全退出 Captain Who 后执行：

```bash
pnpm storage:reset-dev
pnpm storage:reset-dev -- --confirm-reset
```

第一条是非破坏性预检，第二条才会备份并重建。正式工具从 exact current v47 及 exact v35–v46 保留 allowlisted 配置与
credential reference；无法安全识别且含配置的旧库拒绝重置。人机交互设置及 revision 在 v36–v47 reset 中保留，问题与回应历史不保留。通知设置、Browser 下载设置和链接偏好属于配置保留项，通知
事实、浏览/下载记录、Agent 模板与分配、FileChange 事务和会话运行状态不会恢复。应用或 Core Server 仍持锁时
命令会拒绝执行。不要删除原库或手工修改 `PRAGMA user_version`；详见
[存储与数据生命周期](../architecture/storage-and-data-lifecycle.md)。

## 页面一直停留在启动状态

App startup gate 会等待项目、模型和 Electron Host 状态。先定位具体 stage 的错误，不要让 Renderer 用默认值伪装
成成功水合。常见原因包括 Core Server 未就绪、协议 parser 拒绝响应、schema reset required 或模型配置无效。

## Scheduled Automation 未运行或显示 blocked

先确认应用与 Core Server 在计划时间附近持续运行；当前没有操作系统后台调度服务，应用退出期间不会启动 Turn。然后从 Scheduled 页面或 Automation Host API 核对 authoritative task/run：

- 只有 `active` 且 health 为 `ok` 的任务才有 `nextRunAt`；`paused`、`blocked` 或已有非终态 Run 时不会再入队。
- Scheduler 有事件唤醒和 30 秒 fallback scan，因此 due time 不是实时定时器 SLA。离线期间错过的多个 occurrence 会在下次启动合并为一个 `recovery` Run，而不是逐个补跑。
- `queued`/`starting` 可能在等待 Automation 两个并发槽、全局 Turn 容量或现有 Conversation 空闲；可重试容量/占用会以 5 秒 backoff 返回队列。不要把重复点击 `runNow` 当作恢复手段。
- `waiting_for_approval` 不是卡死。打开该 Run 绑定的精确 Conversation/message，处理 durable Approval；不要新建任务代替原 Turn。

`blocked` 会清空 `nextRunAt` 并要求修复配置：

| code                                            | 修复方向                                                |
| ----------------------------------------------- | ------------------------------------------------------- |
| `target_missing/target_archived/target_invalid` | 重新选择可写的根 Conversation，或改为新 Conversation    |
| `project_missing/project_path_missing`          | 重新绑定存在且路径可用的 Project                        |
| `model_missing/model_disabled`                  | 选择存在且启用的 Model                                  |
| `permission_disabled`                           | 重新启用该权限模式，或编辑任务改用当前允许的模式        |
| `schedule_invalid/configuration_invalid`        | 重新编辑 schedule/配置并保存，让 Core Server 重新规范化 |

保存修复时若出现 revision conflict，应先刷新最新 task 再重新应用编辑；不要手工改 SQLite 的 health、revision、Run status 或 lease。完整状态机见 [Scheduled Automation](../subsystems/scheduled-automations.md)。

## 系统通知未出现、语言不对或重复

通用 notification event/batch 现在同时服务普通根任务与 Scheduled Automation。Run、attention、Conversation 与通知是不同事实；
没有系统通知不能推断任务未执行。

1. 先检查 General > Notifications 的总开关、普通任务模式/自定义状态、声音和“显示任务内容”，再检查操作系统通知权限。普通任务的 Never 只关闭四种普通任务状态，不关闭 Automation 类别。
2. Main 启动时立即 drain，之后每 30 秒 polling；Core Server 的 notification event 只用于降低延迟。应用位于前台、原生通知不受支持、通知已禁用或批次已过期时，批次会被 durable suppress，而不是永久留在 pending。
3. 最终校验调用异常或原生 display 失败会释放 claim 并进入有界重试；claim lease 为 60 秒，单批最多尝试 5 次。不要删除或手工重置 outbox/batch 行。
4. Electron 发出 `show` 后 Main 才 ACK；若进程恰在 show 与 ACK 之间崩溃，重启后可能重复显示一次。同一进程内只重试 ACK，不会再次显示。
5. Electron 39 没有稳定的原生 replacement ID。普通批次增量会 ACK 而不再弹一张 toast；只有升级到 attention priority 时允许替换一次。当前没有 Renderer toast fallback。
6. 原生通知语言由 Renderer 的应用语言驱动，Main 仅持有严格校验的 `notification-locale-v1.json` 冷启动镜像。镜像损坏或不受支持时会回退默认语言；重新选择应用语言可让 Renderer 写回，不要手工扩展 locale 文件。

若通知内容已经过期，Electron Main 在显示前触发的 Core Server 最终校验会 suppress 已删除/已读事实、已修复
blocked 状态、已结算 Approval 等 stale delivery。通知恢复与 batch 语义见
[恢复 Runbook](../operations/recovery-runbook.md)。

## `apply_patch` / FileChange 失败

普通文本和代码写入只有 `apply_patch` 这一条模型可见路径。先按错误分类处理，不要改用 shell 重定向、已退役
writer 名称或手工修改数据库绕过 FileChange：

- update/delete 必须带同一 Run 对目标文件最近一次 `read_file` 或成功 `apply_patch` 返回的
  `fileChangeTarget.observationId`；create 不带该 ID，并以原子 no-clobber 证明目标缺失；
- `observation_expired`、`observation_owner_mismatch`、`observation_path_mismatch`、`observation_stale`、`revision_conflict` 或 `conflict` 时，重新读取目标并基于当前内容生成新提案；成功写入返回的 successor observation 可供下一次写入复用；
- Staged 写入每块最多 1 MiB、总内容最多 4 MiB，草稿 7 天过期；mutation index、draft revision 或 owner 不匹配时，从 `status` 获取权威状态，不要重放旧 chunk；
- `outcome_unknown` 表示系统不能证明 publication 结果。先读取权威文件并比对目标 digest；精确目标已存在时按已执行处理，分歧时停止并由用户决定补偿，绝不盲重放；
- symlink、hard link、特殊文件或被交换的父目录会 fail closed。不要通过改变链接结构绕过检查。

审批卡与历史卡中的 diff 来自冻结 FileChange/audit 快照并按需分页；模型 Trace 只保留 body-free digest。若 UI
无法打开 diff，应保留 action/run identity，排查 audit/history 路由，而不是把完整文件内容写进日志。恢复流程见
[恢复 Runbook](../operations/recovery-runbook.md)。

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
- 下载、上传、截图和文件访问经过 Electron Main broker；
- fixed catalog、schema digest 与受管 Playwright 版本一致。

页面加载失败和 guest renderer crash 是两个独立的 Host-owned 状态。使用内置错误页上的 retry/recreate；不要让
Renderer 猜测 URL、复用旧 page generation 或打开 remote debugging 注入结果。下载中心和历史页只接收
path-free 投影；“每次询问保存位置”只适用于用户手动下载，Agent 发起的下载不会弹原生保存对话框。需要交给
Agent 的文件应使用 `browser-download:*` 身份，不要从 Renderer 日志或 SQLite 猜本机绝对路径。

运行发布专项检查见[测试体系](testing.md)。当前 packaged startup 验证仍不等于完整的打包 Agent→MCP
链路通过，不能扩大测试结论。

## Office、PDF 或 Artifact 失败

先运行相应 `verify:*` 命令。Builder/Editor 脚本必须经过 Electron Main 预检并在受管运行时中执行；输出必须通过
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
notarization。若失败发生在 Core Server build，先检查是否只设置了普通 `RUSTFLAGS`；release 构建要求保留
`CARGO_ENCODED_RUSTFLAGS` 并追加 path remap。若失败发生在 afterPack/afterSign，不要修改 frozen receipt 或
扩大签名 allowlist；先定位 privacy gate、目标架构、额外 Mach-O、JIT entitlement、receipt hash 或 signer/Team ID
中的首个差异。`dmg.sign: true` 不替代对最终 DMG 的系统级验签。不要用
`--config.mac.identity=null` 的 unpack 结果冒充可发布安装包。完整步骤见
[构建与发布](build-and-release.md)。

## 仍无法定位

最小化为仓库自有、无网络、无真实凭据的 fixture；记录平台、命令、commit、首个失败和可重复步骤。
如果问题涉及崩溃恢复或不确定副作用，按[恢复 Runbook](../operations/recovery-runbook.md)处理。
