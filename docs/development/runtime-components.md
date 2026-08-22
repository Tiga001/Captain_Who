---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-08-23
---

# 受管运行时组件

本文登记应用依赖的 Core Server sidecar、Office/文档渲染、Artifact Runtime 和 Managed Playwright 组件。版本号以代码常量和锁定 manifest 为准；本表用于审计，不替代 manifest。

## 1. 当前组件清单

| 组件                       | 锁定版本                                 | 开发缓存/来源                      | 打包位置                                 |
| -------------------------- | ---------------------------------------- | ---------------------------------- | ---------------------------------------- |
| Core Server sidecar        | workspace 当前版本，Cargo locked release | `target/release/core-server[.exe]` | `Resources/core-server[.exe]`            |
| OfficeCLI                  | `1.0.139`                                | `.cache/officecli/current`         | `Resources/components/officecli`         |
| Office renderer bundle     | `2026.07.2`                              | `.cache/office-renderer/current`   | `Resources/components/office-renderer`   |
| Office renderer Chromium   | `149.0.7827.55`，revision `1228`         | Office renderer receipt            | 同上                                     |
| Office renderer Playwright | `1.61.1`                                 | Office renderer manifest           | 同上                                     |
| Word/PDF renderer bundle   | `2026.08.1`                              | `.cache/word-pdf-renderer/current` | `Resources/components/word-pdf-renderer` |
| LibreOffice                | `26.2.4.2`                               | Word/PDF renderer receipt          | 同上                                     |
| Artifact Runtime bundle    | `2026.08.4`                              | `.cache/artifact-runtime/current`  | `Resources/components/artifact-runtime`  |
| Artifact managed Node      | `22.23.1`                                | Artifact Runtime receipt           | 同上                                     |
| Artifact managed Python    | `3.12.13+20260610`                       | Artifact Runtime receipt           | 同上                                     |
| Artifact ripgrep           | `15.1.0`                                 | Artifact Runtime receipt           | 同上                                     |
| Managed Playwright MCP     | `@playwright/mcp@0.0.79`                 | pnpm lock / `node_modules`         | `app.asar`                               |
| Managed MCP SDK            | `@modelcontextprotocol/sdk@1.29.0`       | pnpm lock / `node_modules`         | `app.asar`                               |
| Managed Playwright browser | `1.63.0-alpha-2026-08-05`                | pnpm lock / package verifier       | `app.asar`                               |

Office renderer 的 Playwright `1.61.1` 与 Managed Playwright browser 的 `1.63.0-alpha-2026-08-05` 是两个不同用途、独立锁定的 runtime，不应为了“版本看起来一致”而联动升级。

## 2. 平台矩阵

OfficeCLI、Office renderer 和 Artifact Runtime manifest 当前都登记：

- `darwin-arm64`
- `darwin-x64`
- `linux-arm64`
- `linux-x64`
- `win32-arm64`
- `win32-x64`

Word/PDF renderer 也为上述六个 target 锁定 LibreOffice archive/layout。prepare 脚本只允许准备当前 host target，正式 package 必须在目标 OS 原生执行。

“manifest 有 target”只说明 source、size、hash 和布局已登记，不等于该 target 已完成 release acceptance。当前主要实测平台仍是 macOS。

## 3. 准备与验证入口

| 组件               | 准备                             | 只验证当前缓存                  | 脚本测试                              |
| ------------------ | -------------------------------- | ------------------------------- | ------------------------------------- |
| OfficeCLI          | `pnpm prepare:officecli`         | `pnpm verify:officecli`         | `pnpm test:officecli`                 |
| Office renderer    | `pnpm prepare:office-renderer`   | `pnpm verify:office-renderer`   | `pnpm test:office-renderer`           |
| Word/PDF renderer  | `pnpm prepare:word-pdf-renderer` | `pnpm verify:word-pdf-renderer` | `pnpm test:office-renderer`           |
| Artifact Runtime   | `pnpm prepare:artifact-runtime`  | `pnpm verify:artifact-runtime`  | `pnpm test:artifact-runtime`          |
| Core Server        | `pnpm build:core`                | `pnpm verify:core`              | `cargo test -p mycopilot-core-server` |
| Managed Playwright | `pnpm install --frozen-lockfile` | package `afterPack` verifier    | fixed Catalog/bridge/Electron tests   |

开发启动 `pnpm dev` 会准备 OfficeCLI、Office renderer、受支持的 Word/PDF renderer、Artifact Runtime 和 dev icon。package 命令会再次准备目标组件，不能假设旧开发缓存可直接发布。

## 4. Supply-chain 与 receipt 不变量

每个下载组件都必须：

1. 在 repository-owned manifest 中锁定 identity/version、target、URL、exact size 和 SHA-256。
2. 限制下载 host、redirect、总字节与 archive layout；拒绝 path traversal、symlink 或异常文件类型。
3. 下载/解压到 staging，不直接覆盖 `current`。
4. 探测关键 executable 的真实版本。
5. 枚举并 hash 发布文件，生成有界 `component-receipt.json` 或 runtime manifest。
6. 在 staging 完整复验后原子发布；receipt 最后写入，失败保留上一份已验证组件。
7. 打包后尽量重新验证最终 Resources 中的文件和 receipt。
8. 同步第三方 license/notice；不得只复制 executable。

禁止：

- 手工替换 `.cache/*/current` 的单个二进制；
- 使用“latest” URL、浮动 npm range 或未锁定下载；
- 只验证 archive hash 而不验证解压后布局/版本；
- 在 package 签名后再次修改冻结组件字节；
- 把开发者系统已安装的 Office/Node/Python 当发行依赖。

## 5. 各组件职责

### Core Server sidecar

由 `cargo build --locked --release -p mycopilot-core-server --bin core-server` 生成。`verify-core-binary.mjs` 检查文件存在、非空、Unix executable bit 和当前 OS native magic（Mach-O/ELF/PE）。这只证明格式正确，不证明内容 hash 或运行功能；package/startup 测试另行负责。

### OfficeCLI

Office presentation 操作的受管 CLI。manifest schema v1，最大单下载 64 MiB，包含六平台二进制和 LICENSE/NOTICE/THIRD-PARTY-NOTICES。Runtime 不应搜索 PATH 后静默选择另一个版本。

### Office renderer

为 Office HTML/图片渲染和 Vitest browser project 提供冻结 Chromium。manifest schema v2；receipt 绑定 provider、bundle、浏览器/Playwright版本、archive 与全文件 hash，并计算稳定 bundle revision。

### Word/PDF renderer

提供冻结 LibreOffice。不同平台 archive format 为 DMG、tar.gz 或 MSI，prepare 后统一发布到受管 `libreoffice/` 布局，保存 license/notice 并探测 `soffice --version`。

### Artifact Runtime

为文档、表格、演示和 PDF 的受管命令提供封闭依赖：

- Node `22.23.1`；根依赖 `docx@9.6.1`、`exceljs@4.4.0`、`pptxgenjs@4.0.1`；
- Python `3.12.13+20260610`；包括 openpyxl、pdfplumber、pypdf、pypdfium2、python-docx、python-pptx、reportlab、xlsxwriter 的精确版本；
- PDF runtime CLI v1、ripgrep `15.1.0`；
- Node bootstrap/loader、Presentation SDK、pnpm lock、patch、requirements 和 package evidence 的 SHA-256 都是 build inputs。

manifest schema v4。Node package graph 不只验证包名/版本，还通过 frozen evidence 核对文件与依赖图；Python/Rust tool 和 legal inventory 同样进入 receipt。

### Managed Playwright MCP

这是应用 Node dependency，不是 `.cache` 下载组件。`afterPack` 打开 `app.asar`，验证 package name/version、关键入口和 `@playwright/mcp` 对 Playwright/Playwright Core 的精确依赖。

Rust 侧另锁定 69 Tool upstream Catalog 与 reviewed policy digests，当前只暴露 reviewed manifest 允许的 61 个 Tool；Node 包存在不代表全部 upstream Tool 可暴露。执行仍必须经过内置 capability policy、HostBridge、Browser broker 和审批/风险边界。

## 6. Runtime 路径解析

### 开发模式

Main 在没有显式开发 override 时设置：

- `MYCOPILOT_OFFICECLI_PATH` → `.cache/officecli/current/<executable>`
- `MYCOPILOT_OFFICE_RENDERER_DIR` → `.cache/office-renderer/current`
- `MYCOPILOT_WORD_PDF_RENDERER_DIR` → `.cache/word-pdf-renderer/current`
- `MYCOPILOT_ARTIFACT_RUNTIME_DIR` → `.cache/artifact-runtime/current`

开发 override 只应用于显式诊断；测试必须记录 override，不得把开发机上的路径写进持久配置或 release evidence。

### 打包模式

- 未显式配置 OfficeCLI 时，Main 设置 `MYCOPILOT_OFFICE_COMPONENTS_DIR=<Resources>/components`，由 Rust discovery 解析 `officecli`。
- Office renderer 与 Word/PDF renderer 总是替换为应用 Resources 中的固定目录。
- Main 删除继承的 `MYCOPILOT_ARTIFACT_RUNTIME_DIR`，只设置 `MYCOPILOT_ARTIFACT_RUNTIME_COMPONENTS_DIR=<Resources>/components`；Rust 再追加 `artifact-runtime`。
- 普通长生命周期 Core Server 不继承 Office browser proxy 的内部 per-call 环境变量。

这使 package 中的受管 renderer/Artifact Runtime 属于应用信任边界，而不是父 shell 可重定向的路径。

## 7. 打包验证现状

| 组件               | prepare 时验证            | afterPack/afterSign                         | 运行 smoke                                               |
| ------------------ | ------------------------- | ------------------------------------------- | -------------------------------------------------------- |
| Core Server        | native magic + executable | macOS codesign 覆盖；无通用 content receipt | Core Server startup smoke / packaged process observation |
| OfficeCLI          | 完整 receipt              | 当前无专用 packaged verifier hook           | ignored real provider tests                              |
| Office renderer    | 完整 receipt              | afterPack + afterSign                       | browser tests / component smoke                          |
| Word/PDF renderer  | 完整 receipt              | afterPack + afterSign                       | ignored real component tests                             |
| Artifact Runtime   | 完整 supply-chain receipt | 当前无专用 packaged verifier hook           | artifact runtime smoke/ignored real tests                |
| Managed Playwright | lock + manifest/digests   | app.asar package/entry verifier             | unit、stress、Electron E2E、limited packaged startup     |

OfficeCLI、Artifact Runtime 和非 macOS Core Server 的“最终 package 内容复验”是当前缺口，发布文档不得暗示所有 extraResources 都已 afterPack 逐字验证。

## 8. 升级流程

升级一个组件时：

1. 先确认用途与威胁模型，避免把 Office renderer 和 Managed Playwright 当成一个依赖。
2. 更新所有 target 的 URL、size、SHA-256、archive layout、identity/version 与 legal files。
3. 更新 prepare 脚本中的硬编码 pin；manifest 与脚本不一致必须 fail closed。
4. 对 Artifact Runtime 同步 lockfile、package evidence、patch/requirements/build-input hashes。
5. 对 Managed Playwright 同步 npm packages、69 Tool upstream Catalog、61 Tool exposed count、reviewed policy、schema/overlay/digests、Rust 常量和 conformance report。
6. 清理/隔离旧缓存，重新 prepare 并 verify；确认 failure injection 不破坏上一版本。
7. 运行组件脚本测试、真实 smoke、目标 platform package 和 afterPack/afterSign。
8. 更新 third-party notices、本表、构建发布文档和已知限制。

## 9. 代码真源

- manifests：`resources/officecli-manifest.json`、`office-renderer-manifest.json`、`word-pdf-renderer-manifest.json`、`artifact-runtime-manifest.json`
- prepare/verify：`scripts/prepare-officecli.mjs`、`prepare-office-renderer.mjs`、`prepare-word-pdf-renderer.mjs`、`prepare-artifact-runtime.mjs`
- package layout：`electron-builder.yml`、`scripts/verify-packaged-app.mjs`
- runtime discovery：`src/main/core/jsonRpcClient.ts`、`crates/core/src/office`、`crates/core/src/artifact_runtime`
- Core Server build：`package.json`、`scripts/verify-core-binary.mjs`
- Managed Playwright：`package.json`/`pnpm-lock.yaml`、`crates/core-server/src/application/mcp/playwright_manifest.rs`、`crates/core-server/resources/playwright-*.json`

## 10. 测试

```bash
pnpm test:officecli
pnpm test:office-renderer
pnpm test:artifact-runtime
pnpm verify:officecli
pnpm verify:office-renderer
pnpm verify:word-pdf-renderer
pnpm verify:artifact-runtime
pnpm build:core
pnpm test:playwright-fixed-catalog
```

真实 smoke 依赖 prepared component，部分 Rust test 标记 `#[ignore]` 并要求显式环境变量。执行时使用本仓库 `.cache/*/current`，不要指向用户系统安装。发行验收还必须运行目标平台 package 与实际 startup。

## 11. 当前限制

- 组件准备可能联网；仓库没有集中 artifact mirror 或离线 release bundle 流程。
- manifest 覆盖六个平台/架构，但没有 CI 自动逐 target prepare/package/acceptance。
- OfficeCLI 与 Artifact Runtime 缺少最终 package afterPack 完整校验；Core Server 的通用内容 receipt 也未建立。
- `.cache/*/current` 只表示当前已验证组件，没有仓库管理的多版本 cache/rollback catalog。
- Word/PDF 与 Artifact Runtime 体积大，当前文档不提供正式 package size budget。
- Managed Playwright 完整 61-exposed-Tool 行为矩阵和 packaged Agent E2E 仍未完成；69 是 upstream 总数。
- Windows MCP/子进程的完整 process-tree isolation 不因组件存在而得到保证。

## 12. 变更检查表

- [ ] 版本、脚本硬编码 pin、manifest 和 package verifier 是否完全一致？
- [ ] 六个 target 是否都有 exact URL/size/SHA-256/layout，且下载 host/redirect 受限？
- [ ] staging、receipt-last、atomic publish、tamper 和 fault-injection 是否测试？
- [ ] executable 版本、文件清单、依赖图和 legal evidence 是否进入 receipt？
- [ ] runtime discovery 是否区分开发 override 与打包固定路径，且父环境不能重定向受信组件？
- [ ] extraResources/app.asar 中的最终内容是否由 afterPack/afterSign 或 startup gate 验证？
- [ ] macOS/Windows 签名是否避免改变冻结 receipt bytes？
- [ ] Managed Playwright 是否同步 Catalog、policy、digests、bridge 和 conformance report？
- [ ] 是否在目标 OS/arch 完成 prepare、package、真实 smoke 并记录 artifact hash？
- [ ] 是否更新 third-party notices、测试矩阵、构建发布文档和本文？
