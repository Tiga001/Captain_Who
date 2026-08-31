---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-08-31
---

# 测试策略与矩阵

本文说明仓库当前测试入口、层级、发布相关专项 gate 和证据要求。命令以根目录 `package.json`、Cargo workspace 与 `vitest.config.ts` 为准。

## 1. 工具链基线

- Node.js：`>=22 <23`
- pnpm：`11.10.0`
- Rust：workspace toolchain；Clippy、常规测试与专项 Cargo runner 均使用 Cargo lockfile
- Browser tests：使用 `.cache/office-renderer/current` 中 manifest 锁定的 Chromium，不使用开发者系统浏览器
- Electron E2E：真实 Electron fixture 与 Managed Playwright 项目均串行运行，`fileParallelism=false`

首次准备：

```bash
pnpm install --frozen-lockfile
```

需要 Browser tests 时，`pnpm test:browser`（以及聚合入口 `pnpm test:web`）会先执行 `prepare:office-renderer`。`test:web:install` 是普通 Playwright Chromium 安装命令，但当前 Vitest browser project 的可执行真源仍是受管 Office renderer。

## 2. 默认质量入口

```bash
pnpm check
```

它按顺序执行：

1. `pnpm format:check`：Prettier + `cargo fmt --check`
2. `pnpm check:docs`：front matter、单 H1、本地链接/仓库路径、`package.json` scripts、docs 索引、schema/工具链/组件版本等文档漂移检查
3. `pnpm check:public-docs`：公开文档树、元数据、索引、链接边界、必需页面与禁止占位页
4. `pnpm check:test-layout`：检查所有 TypeScript Vitest 文件恰好属于一个 project，并核对 Rust ignored-test registry
5. `pnpm verify:agent-avatars`：确定性检查 Agent avatar 资源未漂移
6. `pnpm lint`：ESLint
7. `pnpm typecheck`：Node + Web TypeScript
8. `pnpm lint:rust`：使用 lockfile 运行 workspace all-targets Clippy，warnings 视为 error
9. `pnpm test`

`pnpm test` 当前包含：

- 测试基础设施 checker 自测；
- OfficeCLI prepare 脚本测试；
- Office renderer 与 Word/PDF renderer prepare/package verifier 测试；
- Core Server release path-remap、macOS signing/frozen Mach-O/privacy/package verifier 的 JavaScript 逻辑测试；
- dev icon、storage reset、Artifact Runtime 脚本测试；
- packaged Playwright startup verifier 与 conformance report 的确定性脚本测试；
- `pnpm test:web`；
- `cargo test --locked --workspace`。

这是一条覆盖广的本地门禁，但不是完整 release gate。

仓库还通过 [`.github/workflows/tests.yml`](../../.github/workflows/tests.yml) 在 pull request、`main` push 和手动触发时并行运行静态检查、脚本测试、Node unit、locked-browser、Linux Rust、macOS Electron fixture、Automation 真实 Core Server E2E 与 Multi-Agent release gate。该 workflow 是源码测试门禁，不构建发布包，也不执行真实签名、notarization 或完整 Managed Playwright release 组合。

## 3. Vitest 项目

`vitest.config.ts` 定义五个项目；文件规则集中在 `scripts/vitest-project-rules.mjs`：

| project                  | 环境                             | 范围                                                                                                            | 并发特点                                   |
| ------------------------ | -------------------------------- | --------------------------------------------------------------------------------------------------------------- | ------------------------------------------ |
| `unit`                   | Node                             | Main、Preload、Renderer 非 browser 测试、`packages/protocol`，排除真实 Electron/Core Server E2E                 | 常规并行                                   |
| `browser`                | Vitest Browser + locked Chromium | App、Chat、Automation、Notification、Skill、MCP/Browser、Git、Sidebar、Files、Collaboration `.browser.test.tsx` | headless 浏览器                            |
| `electron-fixtures`      | 真实 Electron                    | Browser surface failure/manager/target broker 三个 `.electron.test.ts` fixture                                  | 文件串行；仅 macOS 执行用例                |
| `managed-playwright-e2e` | Node 启动真实 Electron fixture   | `managedPlaywrightBridge.electron.test.ts`                                                                      | 文件串行                                   |
| `automation-core-e2e`    | Node + 真实 Core Server 进程     | `automationHostRealCore.integration.test.ts`；Host API → Main → Core Server 的 Automation                       | 文件串行；不启动真实 Electron/系统原生通知 |

执行：

```bash
pnpm test:web
pnpm test:unit
pnpm test:browser
pnpm test:electron
pnpm test:automation-core-e2e
```

`pnpm test:web` 依次聚合 `test:unit`、`test:electron` 和 `test:browser`，让原生 Electron fixture 在 Chromium browser suite 之前获得隔离的冷启动窗口；`test:electron` 再依次运行 `electron-fixtures` 与 `managed-playwright-e2e`，避免两个真实 Electron project 并发争用资源。`check:test-layout` 会拒绝未归属或被多个 project 重复选择的 `src/`、`packages/` TypeScript 测试文件。

修改 Main/Preload/Renderer 跨层功能时，不能只运行 Node unit；至少补 browser 或真实 Electron 边界测试。

## 4. Rust 测试层级

```bash
pnpm test:rust
# 等价于
cargo test --locked --workspace
```

主要层级：

- Rust Core（`mycopilot-core`）单元/集成：领域状态机、Runtime、storage、Tool、Office、Skill、Artifact；
- `mycopilot-core-server` binary 单元：application、transport、bootstrap、Dispatcher、MCP management；
- `crates/core-server/tests/startup_smoke.rs`：真实 stdio 进程启动/协议 smoke；
- `mcp_stdio_runtime_e2e`：真实 Core Server MCP adapter 与 stdio fixture；
- `mycopilot-mcp-client` unit/manager integration/stdio integration；
- `mycopilot-protocol-rs`：共享 fixture 与 Rust DTO wire contract。

两个 MCP stdio target 使用 `harness=false`，同一 repository binary 同时作为 driver 和子 MCP Server。新增模式必须保持离线、确定性和显式 allowlist。

带 `#[ignore]` 的真实组件/压力测试不会被普通 `cargo test` 自动执行。`scripts/ignored-rust-tests.json` 登记其 owner、原因和专项 runner 或 manual 状态；`check:test-layout` 会拒绝源码与登记表的遗漏、过期或重复。专项 runner 必须用 `--ignored --exact` 明确选择，并验证确实运行了预期测试数。

## 5. 脚本与组件测试

| 命令                            | 覆盖                                                                        |
| ------------------------------- | --------------------------------------------------------------------------- |
| `pnpm check:test-layout`        | Vitest project 唯一归属与 Rust ignored-test registry                        |
| `pnpm test:test-infrastructure` | 两个测试布局 checker 的 Node 自测                                           |
| `pnpm test:officecli`           | manifest、下载边界、hash、receipt、原子发布                                 |
| `pnpm test:office-renderer`     | Chromium/LibreOffice prepare 与 packaged verifier                           |
| `pnpm test:artifact-runtime`    | runtime 构建、supply-chain evidence、PPTX notes/SDK                         |
| `pnpm test:mac-signing`         | Core Server path remap、签名/冻结 Mach-O、privacy、pack hooks/verifier 逻辑 |
| `pnpm test:storage-reset-dev`   | Electron 数据根、dry-run/confirm 参数边界                                   |
| `pnpm test:dev-electron-icon`   | 开发图标生成                                                                |

这些 Node tests 多数验证脚本逻辑和 fixture，**不等于**实际下载全部组件、构建 package、使用真实 Developer ID 签名或运行产物。

## 6. 跨语言协议测试

跨进程变更至少同时覆盖：

1. `packages/protocol` TypeScript 类型、runtime parser 和 fixture tests；
2. `crates/protocol-rs` Rust serde/fixture tests；
3. Main `coreServer.*.test.ts` 请求/响应和 notification mapping；
4. Core Server transport tests；
5. 必要时 Preload/Renderer allowlist 与 UI scenario。

当前 `packages/protocol/fixtures` 包含 Agent、Automation、Notification、Collaboration、MCP、Skill 与持久 Agent Run projection 等版本化 JSON
fixture。fixture 是代表性 wire contract，不替代所有 DTO 的双端生成；新增字段必须遵守
required/nullable/default 和 unknown-field 策略。

协议方法名应由 `packages/protocol` 与 `crates/protocol-rs/src/methods.rs` 维护。Main 尚有部分重复字符串，因此相关 test 必须断言精确方法名，直到所有权完全收敛。

## 7. 专项 gate

### FileChange、Notification 与 Browser data

这些新增链路没有单独的 npm release gate，而是进入默认 `unit`/`browser` 与 workspace Cargo tests。改动时应先用下列精确入口定位，再运行 `pnpm check`：

```bash
pnpm exec vitest run --project unit packages/protocol/src/notifications.test.ts packages/protocol/src/browserData.test.ts packages/protocol/src/browserDownloads.test.ts src/main/core/systemNotificationCoordinator.test.ts src/main/core/browserDataIpc.test.ts src/main/core/browserDownloadBroker.test.ts
pnpm exec vitest run --project browser src/renderer/src/features/notifications/__tests__ src/renderer/src/features/mcp/__tests__/McpSettingsPage.browser.test.tsx src/renderer/src/features/rightSidebar/__tests__/BrowserDownloadCenter.browser.test.tsx src/renderer/src/features/chat/__tests__/FileChangeDiffCard.browser.test.tsx
cargo test -p mycopilot-core file_change
cargo test -p mycopilot-core-server file_change
cargo test -p mycopilot-core notification
```

FileChange 的安全证据还包括 source-boundary、权限/Approval、Staged 恢复、run grant、content redaction、durable history 和 crash reconciliation tests。只验证 Renderer diff 卡不能证明文件副作用正确；必须同时覆盖 Rust planner/committer、SQLite authority 和 Core Server settlement。

Notification tests 必须区分 notification fact、聚合 batch 与 native presentation，并覆盖前台/禁用/不支持时 suppress、claim/validate/ACK/release、五次失败上限、优先级升级、locale 与 show/ACK 崩溃窗口。Browser tests 必须使用受管 session 和临时目录，不读取真实 history、下载目录、Cookie 或 profile。

### Scheduled Automation

默认测试已经覆盖 Scheduled Automation 的大部分分层测试：`unit` 包含协议、Main、Preload、通知协调器和 Renderer 状态逻辑，`browser` 包含 Scheduled 页面、表单、Run 历史、导航和 attention，`cargo test --workspace` 包含 schedule、权限投影、SQLite queue/outbox、lease、重启与 Approval 恢复。它们分别经 `test:web` 或 `test:rust` 进入 `pnpm test` 和 `pnpm check`。

真实 Core Server 跨层测试是独立 project，必须显式运行：

```bash
pnpm test:automation-core-e2e
```

**`test:automation-core-e2e` 不在 `test:web`、`test` 或 `check` 中。** 它会构建 debug Core Server，使用临时数据根，并通过生产 Preload bridge、Main IPC registrar 与 Core Server 完成 durable CRUD、revision CAS、`runNow` 入队、Run history 和 attention 调用。其 Electron transport 是进程内适配器，原生通知被模拟为不支持；它不驱动真实定时唤醒、模型完成、Approval 循环、OS 原生通知、进程重启恢复或 packaged Electron。

该专项由 CI 的独立 `automation-real-core` job 自动执行，但仍不改变本地 `pnpm check` 的组成。

修改该子系统时可按层定位：

```bash
pnpm exec vitest run --project unit packages/protocol/src/automations.test.ts src/main/core/coreServer.automation.test.ts src/main/core/ipc.automation.test.ts src/main/core/systemNotificationCoordinator.test.ts src/main/core/ipc.notifications.test.ts src/preload/AutomationIpcBridge.test.ts src/preload/NotificationIpcBridge.test.ts src/renderer/src/features/automations/__tests__
pnpm exec vitest run --project browser src/renderer/src/features/automations/__tests__ src/renderer/src/app/__tests__/LeftSidebarScheduled.browser.test.tsx
cargo test -p mycopilot-core automation_repository
cargo test -p mycopilot-core-server application::automation
pnpm test:automation-core-e2e
```

发布声明和恢复边界见 [Scheduled Automation](../subsystems/scheduled-automations.md)。

### Multi-Agent

```bash
pnpm test:multi-agent-release
```

运行 4 个 profile + 9 个 smoke，并拒绝声称成功但实际筛选到 0 项的 Rust 命令。它不在本地 `pnpm check` 内，但由 CI 的独立 `multi-agent-release` job 自动执行。详细阈值见 [Multi-Agent 发布门禁](../operations/multi-agent-release-gate.md)。

### Managed Playwright

```bash
pnpm test:playwright-fixed-catalog
pnpm test:playwright-round3-stress
pnpm test:playwright-packaged-startup
pnpm verify:playwright-packaged-startup
pnpm test:playwright-conformance-report

# 当前组合入口
pnpm verify:playwright-round3-release
```

注意：组合命令包含对指定 unpacked macOS bundle 的 startup verifier，但不会构建 bundle，也不会证明源码与该 bundle 同一；verifier 返回 `sourceFreshness=not_established_gate_verifies_the_supplied_bundle_only`。

真实 packaged Agent → Managed MCP Server → local fixture 仍为 pending。测试报告中存在 pending 项时，不得把组合入口描述为完整 browser release acceptance。

### MCP stdio 压力

```bash
cargo test -p mycopilot-mcp-client --test stdio_integration
cargo test -p mycopilot-mcp-client --test stdio_integration -- --stress-suite
cargo test -p mycopilot-core-server --test mcp_stdio_runtime_e2e
```

stress 包含 100 次 connect/close、重启、并发 fixture、refresh、call 中 stop/remove、crash recovery 和 1,024/1,025 Tool 边界。官方 MCP Conformance Framework 当前未以锁定 runner 接入，不得把本地通过表述为官方 conformance。

## 8. 当前 `pnpm check` 未覆盖

| 项目                            | 当前状态                                                       |
| ------------------------------- | -------------------------------------------------------------- |
| Automation 真实 Core Server E2E | 独立命令与 CI job，不在 `check`                                |
| Multi-Agent release gate        | 独立命令与 CI job，不在 `check`                                |
| Managed Playwright release组合  | 独立命令，不在 `check`                                         |
| `electron-builder` package      | 不在 `check`                                                   |
| 实际 Developer ID 签名/验签     | 仅真实 `build:mac` 发生；普通 test 只测逻辑                    |
| macOS DMG 实际签名与验证        | `dmg.sign=true` 只在真实 package 发生；仓库无独立 DMG verifier |
| macOS packaged privacy scan     | 真实 macOS afterPack 执行；普通 test 使用 fixture              |
| notarization                    | 未配置                                                         |
| packaged Agent→browser E2E      | pending                                                        |
| Windows/Linux 目标平台验收      | 无仓库 CI 自动运行                                             |

维护者不能因为 `pnpm check` 绿色就声称已完成 release acceptance。

## 9. 测试选择建议

### 快速本地循环

```bash
pnpm format:check
pnpm lint
pnpm typecheck
pnpm exec vitest run --project unit <changed-test-path>
cargo test -p <changed-crate> <test-filter>
```

### 合并前

```bash
pnpm check
```

再按改动域运行专项 gate：Multi-Agent、MCP/Playwright、storage reset、真实组件或目标平台 package。

Scheduled Automation 或其共享 Electron Host/协议/存储链路有变化时，还必须运行 `pnpm test:automation-core-e2e`；不能用默认 `pnpm check` 的绿色替代这条证据。

### 发布前

- 在同一最终 commit 上重新运行 `pnpm check` 与所有适用专项 gate；
- 在目标 OS/arch 实际构建 package；
- 保留命令、环境、完整日志和 artifact hash；
- 不拼接不同 worktree、不同 commit 或陈旧 bundle 的测试结果。

## 10. 测试设计规则

- 优先 fake clock、barrier、receipt 和状态查询；不要使用 sleep 证明并发正确性。
- 临时 SQLite、临时 HOME/appData 和 repository-owned fixture；绝不读写真实用户数据库、浏览器 profile 或凭据。
- 在边界断言 exact identity、sequence、revision、dispatch certainty 和 error code，不解析自然语言文案。
- 故障注入要覆盖事务提交前、提交后通知前和外部副作用不确定窗。
- 负向测试与成功测试同等必要：over-limit、unknown field、duplicate、stale revision、跨树/跨项目、shutdown race。
- 测试输出必须脱敏；使用 canary 证明秘密没有进入 error、trace、event 或 persisted projection。
- snapshot 仅用于稳定展示，不应用来掩盖协议或权限断言。

## 11. 代码真源

- npm scripts：`package.json`
- Vitest projects：`vitest.config.ts`、`scripts/vitest-project-rules.mjs`
- 测试布局检查：`scripts/check-vitest-project-ownership.mjs`、`scripts/check-ignored-rust-tests.mjs`
- Rust ignored-test registry：`scripts/ignored-rust-tests.json`
- 自动源码测试：`.github/workflows/tests.yml`
- Rust targets：workspace `Cargo.toml` 与各 crate `Cargo.toml`
- Multi-Agent runner：`scripts/run-multi-agent-release-gate.mjs`
- Scheduled Automation project：`vitest.config.ts`、`src/main/core/automationHostRealCore.integration.test.ts`
- Scheduled Automation Rust tests：`crates/core-server/src/application/automation`、`crates/core/src/storage/automation_repository/tests.rs`
- Playwright report/gates：`scripts/playwright-conformance-report.mjs`、`verify-packaged-playwright-startup.mjs`
- package hooks tests：`scripts/*.test.mjs`
- public docs checker：`scripts/check-public-docs.mjs`
- FileChange：`crates/core/src/file_change`、`crates/core/src/tools/apply_patch.rs`、`crates/core-server/src/application/agent/tests/file_change_permissions.rs`
- Notification：`packages/protocol/src/notifications.ts`、`crates/core/src/storage/notification_repository.rs`、`src/main/notifications/systemNotificationCoordinator.ts`
- Browser data/download：`packages/protocol/src/browserData.ts`、`packages/protocol/src/browserDownloads.ts`、`src/main/browser`
- cross-language fixtures：`packages/protocol/fixtures`、`packages/protocol/src`、`crates/protocol-rs/src/tests.rs`

## 12. 测试本文变更

修改测试入口或本文后执行：

```bash
pnpm format:check
pnpm check:docs
pnpm check:public-docs
pnpm lint
pnpm typecheck
pnpm check
```

若只调整内部文档且完整 `pnpm check` 成本过高，至少运行 scoped Prettier 与 `pnpm check:docs`；涉及公开文档时还必须运行 `pnpm check:public-docs`。最终合并责任仍应按改动域完成相应 gate。

## 13. 当前限制

- 仓库已有 Linux/macOS 源码测试 workflow；仍没有 Windows job、发布包矩阵、真实签名/notarization job，且 workflow 本身不能证明仓库侧已配置 required check。
- 没有统一 `release:verify` 命令把 `pnpm check`、专项 gate、package、签名和证据绑定起来。
- macOS 是当前主要实测平台；Linux/Windows 的“有 manifest/target”不等于已完成 release acceptance。
- ignored real-component tests 依赖本地 prepared component，不在默认 Cargo 测试中。
- packaged startup verifier 不确认 supplied bundle 与当前源码的新鲜度。
- Managed Playwright 仍缺完整 69-Tool 行为矩阵、重复真实 Electron 压力/RSS gate 和 packaged Agent E2E。
- Scheduled Automation 没有自动化的真实时钟唤醒、真实 Provider/Approval、OS 通知点击、休眠唤醒或 packaged Electron E2E。
- 通用 Notification 没有跨真实目标系统通知中心/锁屏的自动化 E2E；普通 tests 不能证明 OS 展示、声音或点击路由。
- Browser history/download 与 crash recovery 有真实 Electron 局部测试，但仍缺完整 packaged Agent→download→file-input 端到端验收。
- 官方 MCP conformance 未接入锁定 runner。

## 14. 变更检查表

- [ ] 新功能是否同时有 unit、边界集成和必要的真实进程/UI 测试？
- [ ] 新测试是否使用临时数据根、离线 fixture 和脱敏 canary？
- [ ] 新 ignored/profile test 是否有明确 runner，并验证实际执行数量？
- [ ] 跨语言协议是否同时覆盖 Rust、TypeScript、Main/Preload/Renderer？
- [ ] 并发/恢复测试是否使用确定性 barrier 而非放宽 sleep？
- [ ] 新 package/runtime 依赖是否有 prepare、verify、afterPack/afterSign 或 startup 证据？
- [ ] FileChange 是否覆盖 read observation、权限/Approval、CAS、Staged、crash reconciliation、redaction 与历史 diff？
- [ ] Notification 是否覆盖 fact/batch/presentation 分层和 suppress/retry/priority/locale，而未把 native mock 当作 OS E2E？
- [ ] Scheduled Automation 变更是否运行独立 `test:automation-core-e2e`，并将未覆盖的定时、通知和 packaged 场景保留为限制？
- [ ] 是否明确它属于 `pnpm check`、专项 gate、发布必需或仅诊断？
- [ ] 是否更新 `package.json`、本矩阵和相关发布文档，避免命令漂移？
