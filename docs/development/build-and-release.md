---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-09-16
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
- macOS arm64 可更新发行构建需要通过外部环境提供 `CAPTAIN_WHO_UPDATE_URL`；仅允许无凭据、无 query/fragment 的 HTTPS 目录 URL

构建前建议先执行：

```bash
pnpm check
```

`pnpm check` 已包含 `pnpm check:docs`，但不包含专项 release gate、package 或真实签名，不能作为唯一发布证据。

## 2. 构建命令

| 命令                        | 产出/用途                                                                          | 签名语义                                                    |
| --------------------------- | ---------------------------------------------------------------------------------- | ----------------------------------------------------------- |
| `pnpm build`                | TypeScript typecheck + Electron Vite `out/`                                        | 不打包、不签名                                              |
| `pnpm build:core`           | Cargo locked release `core-server`，去除机器私有路径后做 binary magic/execute 校验 | 未签名 sidecar                                              |
| `pnpm build:unpack`         | 准备组件、构建 TS/Core Server、`electron-builder --dir`                            | macOS 显式 `identity=null`，仅本地目录诊断                  |
| `pnpm build:mac`            | macOS arm64 `.app`、DMG、ZIP 与 `latest-mac.yml`；要求配置更新源                   | 强制真实签名；未 notarize                                   |
| `pnpm verify:update-config` | 只验证正式更新源与构建配置，不打包或访问服务器                                     | 不签名、不公证、不上传                                      |
| `pnpm build:win`            | Windows NSIS installer                                                             | 使用 Electron Builder 平台签名配置/环境；仓库未冻结证书流程 |
| `pnpm build:linux`          | AppImage、snap、deb                                                                | 无 macOS 式 code-sign gate                                  |

`build:win/mac/linux` 都按以下顺序执行：准备 OfficeCLI、Office renderer、Word/PDF renderer、Artifact Runtime → TypeScript build → Cargo `--locked --release` Core Server → Electron Builder。

`build:unpack` 中 Word/PDF renderer 使用 `--if-supported`；正式平台命令要求对应 target 可准备。

`build:mac` 在组件准备之前验证更新源，随后以 `scripts/update-config.mjs` 读取基础 YAML、限定
`--mac --arm64` 并显式 `--publish never`。缺少更新地址会提前失败；普通 `build`、`dev` 和使用基础
YAML 的 `build:unpack` 不需要该环境变量，更新保持禁用。更新源只进入最终 app 的
`app-update.yml`，不通过 Vite/renderer 注入第二份 URL。详见[桌面应用更新](../subsystems/desktop-updates.md)。

## 3. 构建流水线

```text
锁定源码/lockfiles/manifests
  → prepare runtime components 到 .cache/*/current
  → verify receipt/hash/identity
  → typecheck + electron-vite build
  → build-core.mjs 追加 Cargo path remap 后执行 locked release build
  → verify native binary magic/executable bit
  → electron-builder files + extraResources
  → afterPack 内容检查
  → Electron fuse 翻转（紧邻 macOS 签名）
  → 平台签名（macOS 有强制策略）
  → afterSign 内容/签名检查
  → installer/image 产物
```

组件 prepare 使用 staging 与 receipt-last/atomic publish。禁止直接修改 `.cache/*/current` 后继续打包；校验失败应重新执行对应 prepare。

`build:core` 不再直接调用 Cargo，而由 `scripts/build-core.mjs` 在保留调用方
`CARGO_ENCODED_RUSTFLAGS` 的同时，为 workspace、Cargo home 和 Rustup home 追加
`--remap-path-prefix`。若调用方只设置普通 `RUSTFLAGS` 而没有等价的 encoded flags，脚本会 fail closed，避免
静默丢失 flags 或把本机绝对路径写入 release binary。Artifact Runtime prepare 同样规范化 Python console-script
shebang；macOS afterPack 还会剥离 packaged `node-pty` Mach-O 的本地 debug/symbol 路径。

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
基础配置显式固定 `asar: true` 与 `disableAsarIntegrity: false`。这不会把 `extraResources` 塞进 archive；它们仍由
receipt、签名和 package gate 约束。

平台产物：

- macOS arm64 可更新发行：`Captain Who.app`、`Captain-Who-${version}-arm64.dmg`、`Captain-Who-${version}-arm64.zip`、`latest-mac.yml` 与对应 blockmap
- Windows：`Captain-Who-${version}-${arch}-Setup.exe`（NSIS）
- Linux：`Captain-Who-${version}-${arch}.AppImage`、snap、deb

实际输出目录和 arch 后缀由 Electron Builder 决定，发布记录应枚举真实文件而不是依赖文档猜测。

## 5. `afterPack` 与 `afterSign`

`electron-builder.yml` 同时把 `scripts/verify-packaged-app.mjs` 注册为 `afterPack` 和 `afterSign`。

### afterPack 当前验证

- OfficeCLI 与 Artifact Runtime 的目标平台/架构、冻结文件集、receipt 和 bundle identity；
- Office renderer packaged receipt/文件；
- Word/PDF renderer packaged receipt/文件；
- `app.asar` 内 Managed Playwright MCP 包和入口：
  - `@playwright/mcp@0.0.79`
  - `@modelcontextprotocol/sdk@1.29.0`
  - `playwright@1.63.0-alpha-2026-08-05`
  - `playwright-core@1.63.0-alpha-2026-08-05`
- macOS `icon.icns` 与源文件字节一致。
- macOS app 全树 privacy gate：拒绝敏感状态文件、绝对 symlink、当前 builder 的私有路径/用户名、高置信 secret 与未精确 allowlist 的 credentialed URL；`app.asar` 会逐 entry 检查 URL fixture。
- 按最终 builder 配置验证更新源：无源/不支持平台不得包含 `app-update.yml`；有源 macOS arm64 必须包含与 generic HTTPS 配置一致且没有附加凭据字段的唯一 `app-update.yml`。
- 所有正式 macOS build 必须使用精确的 Electron fuse 配置、`asar: true` 和 ASAR integrity metadata；该配置由
  `scripts/update-config.mjs` 生成，而非基础 YAML 直接施加到其他平台。即使某次正式 macOS build 不启用更新源，
  签名前 gate 也会拒绝缺少该策略的包。

### afterSign 当前验证

- 再验证 OfficeCLI、Artifact Runtime 与 Office renderer 的签名后 receipt；
- 非 macOS，以及正式签名 macOS，再验证 Word/PDF renderer；macOS `identity=null` 路径不重复该项；
- 正式 macOS 对 app、Core Server、Office renderer、OfficeCLI 和 Artifact Runtime 的精确 native target 执行 strict codesign 与 metadata/entitlement 验证。
- 重新验证上述更新源、架构、DMG+ZIP 和强制签名配置边界。
- 正式 macOS arm64 release 直接读取已签名应用的 Electron fuse wire，要求关闭 `RunAsNode`、`NODE_OPTIONS`/`NODE_EXTRA_CA_CERTS` 和 Node inspector 参数，并启用 embedded ASAR integrity validation 与 only-load-app-from-ASAR。

Core Server 在打包前经过 native magic/execute 校验，正式 macOS afterSign 还会单独 strict verify 该 sidecar；当前 hook 仍没有对非 macOS packaged Core Server 做内容 digest/receipt 绑定。privacy gate 当前也只在 macOS afterPack 执行，Windows/Linux 不能继承该证据。

## 6. macOS 签名契约

`pnpm build:mac` 传入 `forceCodeSigning=true`，`scripts/sign-macos.mjs` 拒绝空 identity、`-` ad-hoc 或 unsigned 配置。

必须满足：

- app code-sign identifier：`io.github.tiga001.captainwho`
- Core Server helper identifier：`io.github.tiga001.captainwho.core-server`
- app 与 Core Server sidecar 使用同一 Team ID 和同一 Developer ID leaf identity
- app 与 Core Server sidecar 都有 hardened runtime 和可信 timestamp
- Core Server designated requirement 绑定稳定 identifier、Apple anchor 和 signer，不能绑定可变 cdhash
- Core Server 使用空 entitlements；应用/继承进程使用 `build/entitlements.mac.plist`
- Office renderer、OfficeCLI 与 Artifact Runtime 只签冻结 receipt 枚举出的完整 Mach-O 集；需要 JIT 的 Chromium、OfficeCLI 和 managed Node 只取得 `build/entitlements.jit-runtime.mac.plist` 中的 `allow-jit`，其他该类目标不带额外 entitlement
- `codesign --verify --deep --strict` 对 app 通过，Core Server sidecar 单独 strict verify 通过
- 对正式 macOS build，`electron-builder` 26.15.3 在签名前紧邻步骤翻转 Electron fuse：关闭
  `ELECTRON_RUN_AS_NODE`、`NODE_OPTIONS`/`NODE_EXTRA_CA_CERTS` 和 Node inspector 参数；开启 embedded
  ASAR integrity validation 与 only-load-app-from-ASAR。afterSign 会读取最终 wire 作为独立证据。

Artifact Runtime、OfficeCLI、Office renderer 和 Word/PDF renderer 目录都被排除出 osx-sign 的第二次递归 pass。自定义 signer 先验证原 receipt，只修改精确 Mach-O allowlist，刷新且只刷新这些 hash，再由顶层 app 签名封装 Resources；LibreOffice 保留其有效上游 Developer ID 签名。任何额外 Mach-O、缺失目标、架构不一致、receipt 外文件变化或 signer identity 分裂都会 fail closed。Windows 同样避免对冻结的 `chrome-headless-shell.exe` 做第二次 Authenticode mutation。

当前 macOS 终端采用 Electron `utilityProcess.fork()`，Core Server/MCP/Artifact Runtime 都不会把 Electron 当作 Node
子进程，因此可以关闭 `RunAsNode`。这项 fuse **只**由 macOS build 动态配置启用：Windows 的 `node-pty`
backend 目前仍使用 Node `child_process.fork()`，在迁移或完成专项回归前不得把 `RunAsNode=false` 扩展到 Windows。

`electron-builder.yml` 还设置 `dmg.sign: true`，因此正式 `build:mac` 会请求 Electron Builder 在生成 block map 前签名 DMG 容器。仓库当前没有独立脚本重新验 DMG 签名；发布证据必须对最终 `.dmg` 另行执行系统级验签，不能只引用 app 的 afterSign 日志。

### Signing 不等于 notarization

当前 `electron-builder.yml` 明确：

```yaml
notarize: false
```

因此 macOS 发行包可以是 Developer ID 已签名，但**尚未经过 Apple notary service**。仓库没有自动
stapling；notary credential 仍由发布者在本机钥匙串管理。基础 YAML 的 `publish: null` 继续禁止默认
provider 推断；动态配置仅为有源 macOS arm64 开启 generic provider、ZIP 与更新 metadata。
NSIS differential package 保持关闭。应用侧 `electron-updater` 集成不意味着公证或更新发布已经完成。
任何发行说明必须如实区分签名、公证和发布状态。

公证/stapling 如果改变了 app 或归档字节，最终 ZIP/DMG、blockmap 与 `latest-mac.yml` 必须根据最终
分发字节重新生成并校验。先上传归档与辅助文件、验证远端摘要，最后发布 manifest；COS 对具体对象
的公开读不要求桶列目录权限。当前尚未配置正式 COS 更新目录或上传、公证与真实 A → B 验收。

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
6. 检查 afterPack/afterSign 完整成功；macOS 保存 signer/Team ID 验证记录和最终 Electron fuse wire 验证记录。
7. macOS 对最终 app 与 DMG 分别验签；公开更新发布前另行完成并记录公证/stapling。当前自动构建仍为 `notarize: false`，不能仅凭构建成功进入公开更新发布。
8. 对实际产物计算并记录 cryptographic hash、大小、平台、架构和版本。
9. 在隔离临时 appData 做启动 smoke；若声明内置浏览器，明确 packaged Agent E2E 仍 pending。
10. 审核第三方 notices、当前限制、schema/reset 和 rollback 说明。
11. 更新发布先上传版本化归档和 blockmap，核对远端最终字节与 manifest 摘要，最后上传 `latest-mac.yml`，并在隔离环境完成真实 A → B 验收。
12. 只有所有必需证据来自同一最终树时才进入发布；任何失败都应修复后完整重跑。

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
- macOS app/managed native target 与 DMG signer、Team ID、notarization 状态；
- privacy gate、Core Server path-remap 与 frozen receipt 校验结果；
- `pnpm check` 和专项 gate 日志；
- 已知 pending/unsupported 项。

回退需要停止有问题版本的分发，并按验证过的数据兼容策略恢复或发布更高版本修复；应用侧更新不提供自动降级或数据库回滚。数据库当前使用 canonical v48，不能假设新库可被旧应用打开。涉及 schema 的 release 必须在发布前明确数据兼容和回滚策略；更新安装不能调用开发用 storage reset。

## 9. 代码真源

- npm build/test scripts：`package.json`
- package 配置：`electron-builder.yml`、`scripts/update-config.mjs`
- 更新源与发布顺序：[桌面应用更新](../subsystems/desktop-updates.md)
- Core Server build/verifier：`scripts/build-core.mjs`、`scripts/verify-core-binary.mjs`
- target OS guard：`scripts/assert-package-platform.mjs`
- package hooks：`scripts/verify-packaged-app.mjs`
- macOS signer/verifier：`scripts/sign-macos.mjs`、`scripts/verify-packaged-macos-signatures.mjs`
- frozen Mach-O/packaged privacy：`scripts/frozen-macho-signing.mjs`、`scripts/verify-packaged-frozen-components.mjs`、`scripts/verify-packaged-privacy.mjs`
- entitlements：`build/entitlements.mac.plist`、`build/entitlements.core-server.mac.plist`、`build/entitlements.jit-runtime.mac.plist`
- component prepare/verify：`scripts/prepare-*.mjs`、`verify-packaged-*.mjs`
- packaged startup：`scripts/verify-packaged-playwright-startup.mjs`

## 10. 测试

```bash
pnpm test:mac-signing
pnpm test:update-config
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

- 已有 Linux/macOS 源码测试 CI，但没有统一 release orchestration、目标平台 package 矩阵、artifact attestation 或自动发布证据归档。
- 已有 macOS arm64 应用侧更新与 generic 源配置，但正式 COS 更新目录、上传、公证/stapling 和真实 A → B 发布验收尚未完成；没有自动降级或数据库回滚通道。
- Windows/Linux 没有仓库内目标平台 release acceptance 记录；目标定义不等于已验证。
- 非 macOS afterPack 没有等价的全树 privacy scan；全部平台仍缺 packaged Core Server content digest/receipt 绑定。
- Electron Builder 已请求签名 DMG，但仓库没有独立 DMG signature verifier，也没有 notarization/stapling。
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
- [ ] frozen component 是否枚举完整 Mach-O 集、仅刷新被签文件 hash，并阻止 osx-sign 第二次修改 receipt 边界？
- [ ] package 是否通过私有路径、敏感状态文件、secret/credentialed URL 与绝对 symlink 检查；非 macOS 缺口是否明确？
- [ ] 是否明确 signing、notarization、publishing、updating 四种不同状态？
- [ ] `pnpm check`、专项 gate、目标 package 和 startup 证据是否来自同一最终 commit？
- [ ] Scheduled Automation 变更是否另跑 `test:automation-core-e2e`，并在目标 package 上记录未自动覆盖的调度、Approval、恢复与通知 smoke？
- [ ] 是否记录 artifact hash、平台/arch、签名 identity 与所有 pending 项？
- [ ] schema 变更是否评估旧应用回滚和开发 reset 行为？
- [ ] 是否同步更新运行时组件、测试矩阵和本文？
