---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-08-23
---

# 测试策略与矩阵

本文说明仓库当前测试入口、层级、发布相关专项 gate 和证据要求。命令以根目录 `package.json`、Cargo workspace 与 `vitest.config.ts` 为准。

## 1. 工具链基线

- Node.js：`>=22 <23`
- pnpm：`11.10.0`
- Rust：workspace toolchain；发布构建使用 Cargo lockfile
- Browser tests：使用 `.cache/office-renderer/current` 中 manifest 锁定的 Chromium，不使用开发者系统浏览器
- Electron E2E：Managed Playwright 项目串行运行，`fileParallelism=false`

首次准备：

```bash
pnpm install --frozen-lockfile
```

需要 Browser tests 时，`pnpm test:web` 会先执行 `prepare:office-renderer`。`test:web:install` 是普通 Playwright Chromium 安装命令，但当前 Vitest browser project 的可执行真源仍是受管 Office renderer。

## 2. 默认质量入口

```bash
pnpm check
```

它按顺序执行：

1. `pnpm format:check`：Prettier + `cargo fmt --check`
2. `pnpm check:docs`：front matter、单 H1、本地链接/仓库路径、`package.json` scripts、docs 索引、schema/工具链/组件版本等文档漂移检查
3. `pnpm lint`：ESLint
4. `pnpm typecheck`：Node + Web TypeScript
5. `pnpm lint:rust`：workspace all-targets Clippy，warnings 视为 error
6. `pnpm test`

`pnpm test` 当前包含：

- OfficeCLI prepare 脚本测试；
- Office renderer 与 Word/PDF renderer prepare/package verifier 测试；
- macOS signing/package verifier 的 JavaScript 逻辑测试；
- dev icon、storage reset、Artifact Runtime 脚本测试；
- `pnpm test:web`；
- `cargo test --workspace`。

这是一条覆盖广的本地门禁，但不是完整 release gate。

## 3. Vitest 项目

`vitest.config.ts` 定义三个项目：

| project                  | 环境                             | 范围                                                                          | 并发特点                                 |
| ------------------------ | -------------------------------- | ----------------------------------------------------------------------------- | ---------------------------------------- |
| `unit`                   | Node                             | Main、Preload、Renderer 非 browser 测试、`packages/protocol`                  | 常规并行；排除真实 Electron managed test |
| `browser`                | Vitest Browser + locked Chromium | App、Chat、Skill、MCP、Git、Sidebar、Files、Collaboration `.browser.test.tsx` | headless 浏览器                          |
| `managed-playwright-e2e` | Node 启动真实 Electron fixture   | `managedPlaywrightBridge.electron.test.ts`                                    | 文件串行                                 |

执行：

```bash
pnpm test:web
pnpm exec vitest run --project unit
pnpm exec vitest run --project browser
pnpm exec vitest run --project managed-playwright-e2e
```

修改 Main/Preload/Renderer 跨层功能时，不能只运行 Node unit；至少补 browser 或真实 Electron 边界测试。

## 4. Rust 测试层级

```bash
pnpm test:rust
# 等价于
cargo test --workspace
```

主要层级：

- Rust Core（`mycopilot-core`）单元/集成：领域状态机、Runtime、storage、Tool、Office、Skill、Artifact；
- `mycopilot-core-server` binary 单元：application、transport、bootstrap、Dispatcher、MCP management；
- `crates/core-server/tests/startup_smoke.rs`：真实 stdio 进程启动/协议 smoke；
- `mcp_stdio_runtime_e2e`：真实 Core Server MCP adapter 与 stdio fixture；
- `mycopilot-mcp-client` unit/manager integration/stdio integration；
- `mycopilot-protocol-rs`：共享 fixture 与 Rust DTO wire contract。

两个 MCP stdio target 使用 `harness=false`，同一 repository binary 同时作为 driver 和子 MCP Server。新增模式必须保持离线、确定性和显式 allowlist。

带 `#[ignore]` 的真实组件/压力测试不会被普通 `cargo test` 自动执行。专项 runner 必须用 `--ignored --exact` 明确选择，并验证确实运行了预期测试数。

## 5. 脚本与组件测试

| 命令                          | 覆盖                                                 |
| ----------------------------- | ---------------------------------------------------- |
| `pnpm test:officecli`         | manifest、下载边界、hash、receipt、原子发布          |
| `pnpm test:office-renderer`   | Chromium/LibreOffice prepare 与 packaged verifier    |
| `pnpm test:artifact-runtime`  | runtime 构建、supply-chain evidence、PPTX notes/SDK  |
| `pnpm test:mac-signing`       | signer 参数、pack hooks、签名 metadata verifier 逻辑 |
| `pnpm test:storage-reset-dev` | Electron 数据根、dry-run/confirm 参数边界            |
| `pnpm test:dev-electron-icon` | 开发图标生成                                         |

这些 Node tests 多数验证脚本逻辑和 fixture，**不等于**实际下载全部组件、构建 package、使用真实 Developer ID 签名或运行产物。

## 6. 跨语言协议测试

跨进程变更至少同时覆盖：

1. `packages/protocol` TypeScript 类型、runtime parser 和 fixture tests；
2. `crates/protocol-rs` Rust serde/fixture tests；
3. Main `coreServer.*.test.ts` 请求/响应和 notification mapping；
4. Core Server transport tests；
5. 必要时 Preload/Renderer allowlist 与 UI scenario。

当前 `packages/protocol/fixtures` 包含 Agent、Collaboration、MCP、Skill 等版本化 JSON fixture。fixture 是代表性 wire contract，不替代所有 DTO 的双端生成；新增字段必须遵守 required/nullable/default 和 unknown-field 策略。

协议方法名应由 `packages/protocol` 与 `crates/protocol-rs/src/methods.rs` 维护。Main 尚有部分重复字符串，因此相关 test 必须断言精确方法名，直到所有权完全收敛。

## 7. 专项 gate

### Multi-Agent

```bash
pnpm test:multi-agent-release
```

运行 4 个 profile + 9 个 smoke。它不在 `pnpm check` 内。详细阈值见 [Multi-Agent 发布门禁](../operations/multi-agent-release-gate.md)。

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

| 项目                           | 当前状态                                    |
| ------------------------------ | ------------------------------------------- |
| Multi-Agent release gate       | 独立命令，不在 `check`                      |
| Managed Playwright release组合 | 独立命令，不在 `check`                      |
| `electron-builder` package     | 不在 `check`                                |
| 实际 Developer ID 签名/验签    | 仅真实 `build:mac` 发生；普通 test 只测逻辑 |
| notarization                   | 未配置                                      |
| packaged Agent→browser E2E     | pending                                     |
| Windows/Linux 目标平台验收     | 无仓库 CI 自动运行                          |

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
- Vitest projects：`vitest.config.ts`
- Rust targets：workspace `Cargo.toml` 与各 crate `Cargo.toml`
- Multi-Agent runner：`scripts/run-multi-agent-release-gate.mjs`
- Playwright report/gates：`scripts/playwright-conformance-report.mjs`、`verify-packaged-playwright-startup.mjs`
- package hooks tests：`scripts/*.test.mjs`
- cross-language fixtures：`packages/protocol/fixtures`、`packages/protocol/src`、`crates/protocol-rs/src/tests.rs`

## 12. 测试本文变更

修改测试入口或本文后执行：

```bash
pnpm format:check
pnpm check:docs
pnpm lint
pnpm typecheck
pnpm check
```

若只调整文档且完整 `pnpm check` 成本过高，至少运行 `pnpm format:check` 与 `pnpm check:docs`；最终合并责任仍应按改动域完成相应 gate。

## 13. 当前限制

- 仓库没有 `.github` CI workflow，也没有自动平台矩阵或 required check。
- 没有统一 `release:verify` 命令把 `pnpm check`、专项 gate、package、签名和证据绑定起来。
- macOS 是当前主要实测平台；Linux/Windows 的“有 manifest/target”不等于已完成 release acceptance。
- ignored real-component tests 依赖本地 prepared component，不在默认 Cargo 测试中。
- packaged startup verifier 不确认 supplied bundle 与当前源码的新鲜度。
- Managed Playwright 仍缺完整 69-Tool 行为矩阵、重复真实 Electron 压力/RSS gate 和 packaged Agent E2E。
- 官方 MCP conformance 未接入锁定 runner。

## 14. 变更检查表

- [ ] 新功能是否同时有 unit、边界集成和必要的真实进程/UI 测试？
- [ ] 新测试是否使用临时数据根、离线 fixture 和脱敏 canary？
- [ ] 新 ignored/profile test 是否有明确 runner，并验证实际执行数量？
- [ ] 跨语言协议是否同时覆盖 Rust、TypeScript、Main/Preload/Renderer？
- [ ] 并发/恢复测试是否使用确定性 barrier 而非放宽 sleep？
- [ ] 新 package/runtime 依赖是否有 prepare、verify、afterPack/afterSign 或 startup 证据？
- [ ] 是否明确它属于 `pnpm check`、专项 gate、发布必需或仅诊断？
- [ ] 是否更新 `package.json`、本矩阵和相关发布文档，避免命令漂移？
