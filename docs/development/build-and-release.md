---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-08-23
---

# 构建与发布

本文记录当前 Electron + Core Server sidecar 的构建、打包、macOS 签名和发布证据边界。当前没有一键发布流水线；维护者必须明确区分“源码构建”“目录包”“发行包”“已签名”和“已公证”。

## 1. 前置条件

- Node.js `>=22 <23`
- pnpm `11.10.0`
- Rust/Cargo workspace toolchain
- `pnpm install --frozen-lockfile`
- 组件准备阶段需要访问各锁定 manifest 指定的下载源；已完整缓存且 receipt 有效时可复用
- 目标平台 package 必须在目标 OS 原生构建；脚本会拒绝在其他 OS 执行 `build:win/mac/linux`
- macOS distribution 需要钥匙串中可用的真实 `Developer ID Application` identity

构建前建议先执行：

```bash
pnpm check
```

`pnpm check` 已包含 `pnpm check:docs`，但不包含专项 release gate、package 或真实签名，不能作为唯一发布证据。

## 2. 构建命令

| 命令                | 产出/用途                                                      | 签名语义                                                    |
| ------------------- | -------------------------------------------------------------- | ----------------------------------------------------------- |
| `pnpm build`        | TypeScript typecheck + Electron Vite `out/`                    | 不打包、不签名                                              |
| `pnpm build:core`   | Cargo locked release `core-server` + binary magic/execute 校验 | 未签名 sidecar                                              |
| `pnpm build:unpack` | 准备组件、构建 TS/Core Server、`electron-builder --dir`        | macOS 显式 `identity=null`，仅本地目录诊断                  |
| `pnpm build:mac`    | macOS `.app`/DMG                                               | 强制真实签名；未 notarize                                   |
| `pnpm build:win`    | Windows NSIS installer                                         | 使用 Electron Builder 平台签名配置/环境；仓库未冻结证书流程 |
| `pnpm build:linux`  | AppImage、snap、deb                                            | 无 macOS 式 code-sign gate                                  |

`build:win/mac/linux` 都按以下顺序执行：准备 OfficeCLI、Office renderer、Word/PDF renderer、Artifact Runtime → TypeScript build → Cargo `--locked --release` Core Server → Electron Builder。

`build:unpack` 中 Word/PDF renderer 使用 `--if-supported`；正式平台命令要求对应 target 可准备。

## 3. 构建流水线

```text
锁定源码/lockfiles/manifests
  → prepare runtime components 到 .cache/*/current
  → verify receipt/hash/identity
  → typecheck + electron-vite build
  → cargo build --locked --release core-server
  → verify native binary magic/executable bit
  → electron-builder files + extraResources
  → afterPack 内容检查
  → 平台签名（macOS 有强制策略）
  → afterSign 内容/签名检查
  → installer/image 产物
```

组件 prepare 使用 staging 与 receipt-last/atomic publish。禁止直接修改 `.cache/*/current` 后继续打包；校验失败应重新执行对应 prepare。

## 4. 打包内容

Electron Builder 将 `out/**`、`resources/**` 和所需 Node modules 放入应用包，并将下列内容作为 `extraResources`：

| 来源                               | 产物位置                                 |
| ---------------------------------- | ---------------------------------------- |
| `.cache/officecli/current`         | `Resources/components/officecli`         |
| `.cache/office-renderer/current`   | `Resources/components/office-renderer`   |
| `.cache/word-pdf-renderer/current` | `Resources/components/word-pdf-renderer` |
| `.cache/artifact-runtime/current`  | `Resources/components/artifact-runtime`  |
| `target/release/core-server[.exe]` | `Resources/core-server[.exe]`            |
| third-party/Electron notices       | `Resources/` 相应 notices/licenses       |

`resources/**` 被 `asarUnpack`；Managed Playwright 的 Node packages 则由 `app.asar` 内容校验确认版本与入口存在。

平台产物：

- macOS：`MyCopilot.app` 与 `${name}-${version}.dmg`
- Windows：`${name}-${version}-setup.exe`（NSIS）
- Linux：`${name}-${version}.AppImage`、snap、deb

实际输出目录和 arch 后缀由 Electron Builder 决定，发布记录应枚举真实文件而不是依赖文档猜测。

## 5. `afterPack` 与 `afterSign`

`electron-builder.yml` 同时把 `scripts/verify-packaged-app.mjs` 注册为 `afterPack` 和 `afterSign`。

### afterPack 当前验证

- Office renderer packaged receipt/文件；
- Word/PDF renderer packaged receipt/文件；
- `app.asar` 内 Managed Playwright MCP 包和入口：
  - `@playwright/mcp@0.0.79`
  - `@modelcontextprotocol/sdk@1.29.0`
  - `playwright@1.63.0-alpha-2026-08-05`
  - `playwright-core@1.63.0-alpha-2026-08-05`
- macOS `icon.icns` 与源文件字节一致。

### afterSign 当前验证

- 再验证 Office renderer；
- 非 macOS 以及未显式禁签的 macOS 再验证 Word/PDF renderer；macOS `identity=null` 路径不在 afterSign 重复该项；
- macOS 在未显式禁签时执行真实 codesign metadata 验证。

当前 hook **没有独立验证打包后 OfficeCLI、Artifact Runtime 或 Core Server sidecar 的完整 receipt/content**。Core Server 在打包前只经过 native magic/execute 校验；macOS Core Server sidecar 会在 afterSign 进入 codesign 验证。此缺口应在扩大正式发布声明前补齐。

## 6. macOS 签名契约

`pnpm build:mac` 传入 `forceCodeSigning=true`，`scripts/sign-macos.mjs` 拒绝空 identity、`-` ad-hoc 或 unsigned 配置。

必须满足：

- app code-sign identifier：`com.mycopilot.next`
- Core Server helper identifier：`com.mycopilot.next.core-server`
- app 与 Core Server sidecar 使用同一 Team ID 和同一 Developer ID leaf identity
- app 与 Core Server sidecar 都有 hardened runtime 和可信 timestamp
- Core Server designated requirement 绑定稳定 identifier、Apple anchor 和 signer，不能绑定可变 cdhash
- Core Server 使用空 entitlements；应用/继承进程使用 `build/entitlements.mac.plist`
- `codesign --verify --deep --strict` 对 app 通过，Core Server sidecar 单独 strict verify 通过

Office/Word renderer 中的冻结 Chromium/LibreOffice 内容在配置指定位置避免递归重签改变 receipt bytes；顶层 app 签名仍封装 Resources。Windows 同样避免对冻结的 `chrome-headless-shell.exe` 做第二次 Authenticode mutation。

### Signing 不等于 notarization

当前 `electron-builder.yml` 明确：

```yaml
notarize: false
```

因此 macOS 发行包可以是 Developer ID 已签名，但**尚未经过 Apple notary service**。仓库也没有 stapling、notary credential、自动 publish 或 updater channel。任何发行说明必须如实区分这些状态。

## 7. 发布前门禁

当前没有单一 `release:verify`，建议按同一最终 commit 依次记录：

1. 确认目标 release commit/worktree 与 lockfiles；记录 `git rev-parse HEAD` 和工作树状态。
2. `pnpm install --frozen-lockfile`。
3. `pnpm check`。
4. 按改动域运行专项 gate：
   - `pnpm test:automation-core-e2e`
   - `pnpm test:multi-agent-release`
   - `pnpm verify:playwright-round3-release`
   - MCP stdio stress/其他明确发布测试
5. 在目标 OS/arch 执行 `pnpm build:<platform>`；不得跨 OS 伪装 native package。
6. 检查 afterPack/afterSign 完整成功；macOS 保存 signer/Team ID 验证记录。
7. 对实际产物计算并记录 cryptographic hash、大小、平台、架构和版本。
8. 在隔离临时 appData 做启动 smoke；若声明内置浏览器，明确 packaged Agent E2E 仍 pending。
9. 审核第三方 notices、当前限制、schema/reset 和 rollback 说明。
10. 只有所有必需证据来自同一最终树时才进入发布；任何失败都应修复后完整重跑。

不要把 `build:unpack` 的 unsigned directory、脚本 unit test、旧 bundle startup 或不同 commit 的 gate 拼接成签名发布证据。

### Scheduled Automation 发布验证

Scheduled Automation 或其协议、Host API、Core Server、SQLite schema、权限/Approval、系统通知链路有变化时，发布证据至少包括：

```bash
pnpm check
pnpm test:automation-core-e2e
pnpm build:<platform>
```

`pnpm check` 会经 `test:web` 和 `test:rust` 覆盖 Automation 的 unit、Browser、调度/lease、权限、Approval restart 和 durable outbox 测试；独立的 `test:automation-core-e2e` **不在** `test:web`、`test` 或 `check` 内，必须另行记录。它只证明 debug Core Server 上的 durable CRUD、CAS、`runNow` 入队、history 和 attention 跨层调用，不证明真实定时唤醒、Provider 完成、OS 通知、休眠唤醒、重启后的真实进程恢复或 packaged Electron。

如果发行说明声称目标平台支持 Scheduled Automation，应在该平台的最终 package 上另做人工 smoke，并把它明确标记为人工证据：

- 使用隔离的新数据根创建/编辑/暂停任务，验证 active、blocked 与 attention 投影；
- 以真实短周期任务验证 due Run 只创建一个根 Agent Turn，并检查 Conversation/Run identity；
- 用默认权限走一次 `waiting_for_approval`，确认批准或拒绝继续同一 Turn；
- 重启应用后核对 overdue Run 标为 `recovery`、已绑定 Run 不被重复启动；
- 在系统允许和拒绝原生通知两种状态下核对 durable attention/outbox；允许时再验证通知点击打开精确任务或 Conversation/message。

仓库目前没有自动化 OS/packaged Scheduled Automation E2E，也没有一条脚本完成上述人工验收。不得把 unit 中模拟的 `Notification`、进程内 Electron transport 或 unpacked 启动当作目标平台通知/packaged 验收。子系统边界见 [Scheduled Automation](../subsystems/scheduled-automations.md)。

## 8. 产物验收与回滚

发布记录至少保存：

- source commit、package version、Node/pnpm/Rust 版本；
- OS、arch、Electron Builder 命令；
- component manifest/receipt identity；
- package 文件名、大小、SHA-256；
- macOS signer、Team ID、notarization 状态；
- `pnpm check` 和专项 gate 日志；
- 已知 pending/unsupported 项。

回滚当前只能发布前一已验证产物或停止分发；仓库没有自动更新/回滚服务。数据库又采用开发 reset-only policy，不能假设新版本写入的 v23 后继库可被旧应用打开。涉及 schema 的 release 必须在发布前明确数据兼容和回滚策略。

## 9. 代码真源

- npm build/test scripts：`package.json`
- package 配置：`electron-builder.yml`
- Core Server verifier：`scripts/verify-core-binary.mjs`
- target OS guard：`scripts/assert-package-platform.mjs`
- package hooks：`scripts/verify-packaged-app.mjs`
- macOS signer/verifier：`scripts/sign-macos.mjs`、`verify-packaged-macos-signatures.mjs`
- entitlements：`build/entitlements.mac.plist`、`build/entitlements.core-server.mac.plist`
- component prepare/verify：`scripts/prepare-*.mjs`、`verify-packaged-*.mjs`
- packaged startup：`scripts/verify-packaged-playwright-startup.mjs`

## 10. 测试

```bash
pnpm test:mac-signing
pnpm test:officecli
pnpm test:office-renderer
pnpm test:artifact-runtime
pnpm test:playwright-packaged-startup
pnpm test:automation-core-e2e
pnpm build:core
```

实际发行还必须运行目标平台 `pnpm build:<platform>`。JavaScript signer/verifier tests 只验证逻辑分支，不能替代真实证书、codesign、installer 和目标系统启动。

macOS 可在已有 fresh unpacked bundle 上执行：

```bash
pnpm verify:playwright-packaged-startup
```

该命令不构建 bundle，也不建立 source freshness，且不驱动 Agent MCP Tool。

## 11. 当前限制

- 没有 CI、统一 release orchestration、artifact attestation 或自动证据归档。
- 没有 notarization、stapling、自动发布、更新或回滚通道。
- Windows/Linux 没有仓库内目标平台 release acceptance 记录；目标定义不等于已验证。
- afterPack 没有独立校验 packaged OfficeCLI、Artifact Runtime 与全部 Core Server content。
- packaged startup gate 仅支持 macOS unpacked app，且 supplied bundle freshness 不确定。
- Packaged Agent → Managed Playwright MCP E2E 为 pending。
- Scheduled Automation 缺真实时钟、Provider/Approval、OS 通知点击、重启/休眠与 packaged Electron 自动化 E2E。
- `pnpm check` 不包含 package、真实签名、Automation 真实 Core Server E2E、Multi-Agent 或 Playwright 专项 gate。

## 12. 变更检查表

- [ ] Node/pnpm/Rust/lockfile 与目标平台前置条件是否更新？
- [ ] 新 runtime/sidecar 是否有锁定 manifest、hash、receipt、合法来源和 third-party notices？
- [ ] prepare 是否 staging + atomic publish，package hook 是否验证最终产物而非只验证缓存？
- [ ] 新 extraResource 是否在所有平台路径、asar 策略和 runtime discovery 中一致？
- [ ] macOS 新 executable 是否有稳定 identifier、最小 entitlements、hardened runtime 和实际验签？
- [ ] 是否明确 signing、notarization、publishing、updating 四种不同状态？
- [ ] `pnpm check`、专项 gate、目标 package 和 startup 证据是否来自同一最终 commit？
- [ ] Scheduled Automation 变更是否另跑 `test:automation-core-e2e`，并在目标 package 上记录未自动覆盖的调度、Approval、恢复与通知 smoke？
- [ ] 是否记录 artifact hash、平台/arch、签名 identity 与所有 pending 项？
- [ ] schema 变更是否评估旧应用回滚和开发 reset 行为？
- [ ] 是否同步更新运行时组件、测试矩阵和本文？
