---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-23
---

# 浏览器与自动化

该子系统让用户手动浏览网页，并让 Agent 在同一组受管 Browser surfaces 上调用内置 Managed Playwright
MCP Tool。这里的“自动化”只指浏览器控制，不是 Scheduled 中的 Agent 定时任务；后者见
[Scheduled Automation](./scheduled-automations.md)。两条浏览器路径共享 guest、Session 和 Main 安全
策略，但授权语义不同。右侧栏页面生命周期见 [右侧栏平台](./right-sidebar.md)，MCP Server 管理见
[MCP 子系统](./mcp.md)。

## 职责边界

| 层                      | 当前职责                                                                                                                                                  |
| ----------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Renderer `BrowserPanel` | 地址栏、前进/后退/刷新、缩放、清除浏览数据、页面标题/favicon；创建受管 `<webview>`；报告可见 surface 和 viewport                                          |
| 右侧栏平台              | Browser 多页面、keep-alive、全局上下文；响应 Main 的 reveal/create/select/resize/close 命令                                                               |
| Electron Main           | guest 附加与安全配置；持久 Session；surface/instance/selection 身份；Target/CDP；网络 Guard；风险协调；上传/下载/Artifact Broker；Managed Playwright Host |
| Rust Core               | 内置 capability 启用策略；Agent 授权和敏感 Tool 审批；反向桥请求；风险批准；调用生命周期与持久事件                                                        |
| Managed Playwright Host | 固定、审查过的官方 Tool 目录与执行；只通过 Main 提供的受管 BrowserContext 工作                                                                            |

Renderer 不获得 guest WebContents id、CDP target id、调试端点、目标绑定 token、原始文件路径或 Artifact 托管路径。Rust Core 不直接控制 Electron WebContents；Main 不自行授予 Agent 权限。

## 手动 Browser surface

每个右侧栏 Browser page 有稳定 `surfaceId`，由 page id 派生。Renderer 使用仅含该 id 的 inert bootstrap URL：

```text
about:blank#mycopilot-browser-surface=<encoded-surface-id>
```

Main 在 `will-attach-webview` 校验 partition 与初始 URL、收紧 webPreferences；`did-attach-webview` 后从 bootstrap URL 识别 surface。无法在 2 秒内得到合法且唯一身份的 guest 会 fail closed 并关闭。

一次 guest incarnation 由 Main 生成 `surfaceInstanceId`。Surface 就绪流程必须匹配：

- 当前 Main command 的 `requestId`
- 稳定 `surfaceId`
- 精确 `surfaceInstanceId`
- 可选、受限的 viewport

结果区分 `applied`、`noop` 与 `stale`。`not_registered`/`instance_mismatch` 可在严格预算内重新探测；expired、superseded、cancelled 或 closed 请求不能重放。

## 可见页面选择

页面被选中时，Renderer 先发送不带 instance 的 side-effect-free probe。Main 返回当前 opaque instance 后，Renderer 仅在同一个 DOM webview 仍为当前页面时回显。每次 intent 使用严格递增 `selectionRevision`；Main 返回 `authoritativeRevision` 并拒绝 stale revision、instance mismatch 或 closing surface。

没有 Browser 页处于 foreground 时，Renderer 用更高 revision 明确清空选择。UI 选择只说明哪个受管 surface 是当前交互目标，不授予 debugger 或 Agent 自动化权限。

Main → Renderer 的 surface command 包括：

- `ensureAttached`
- `createSurface`
- `selectSurface`
- `resizeSurface`
- `closeSurface`

命令由 AppShell 级 bridge 接收并打开右侧栏。Preload 严格解析事件；Renderer 只负责布局和确认 ready。

## Webview 与 Session 安全

Browser 使用固定持久 partition `persist:mycopilot-browser`，因此 cookies、站点存储和缓存跨标签及应用会话保留，直到用户清除浏览数据。

Main 对 guest 强制：

- 仅登记的 partition 可附加。
- 删除 preload；关闭 Node integration、worker/subframe Node、plugins、experimental features 和嵌套 webview。
- 开启 sandbox、context isolation、webSecurity、safeDialogs；禁止 drag/drop navigation 和不安全内容。
- 网页权限 check/request 一律拒绝。
- 页面协议只允许 HTTP/HTTPS；bootstrap `about:blank` 和内部 Chromium PDF viewer 走窄例外。
- Electron popup 本身永远拒绝。当前应用在 Electron Main 的导航与网络策略允许后，为合法 `target=_blank` 创建一个非活动的受管新 surface；网页不会获得真实 popup WebContents，也不会直接决定 surface identity。
- 非法导航/redirect 被阻止；日志不打印可能含 query/credential 的完整 URL。

清除浏览数据同时清理该 Session 的 cache/storage 和 favicon cache。

## 内置 `browser_automation` capability

该能力是 Rust Core 注册的内置 MCP capability，不是用户配置的 stdio MCP Server。设置页从 Rust Core 获取 capability 列表，使用精确 `policyRevision` 做 CAS 开关。Renderer 的开关仅表达用户策略；每次 Agent 激活和敏感调用仍由 Rust Core 验证。

Rust Core 通过反向 JSON-RPC 通知 Main 执行 `ManagedPlaywrightCommand`：

1. `connect`
2. 分页 `list_tools`
3. `prepare_sensitive_tool`
4. `call_tool`
5. `release_sensitive_tool_binding`
6. `close`

Main 只加载固定 manifest/catalog 中审查过的官方 Tool。当前锁定 `@playwright/mcp` 0.0.79 的完整 69-Tool upstream catalog，其中 61 个对 Agent 暴露：25 个 `pass_through`、8 个 `host_adapted`、21 个 `approval_required`、7 个 `artifact_managed`；另有 7 个 `unsupported` 和 1 个未暴露的 `sandboxed` Tool。未知 Tool、catalog digest 漂移、越界参数和过大输出 fail closed。`ManagedPlaywrightBridgeHost` 将 request/deadline/cancel 与一个 MCP Server generation 和 Electron Main Host generation 绑定，迟到 completion 不得满足新请求。

## 目标绑定与敏感 Tool

普通 capability grant 不足以调用敏感 Tool。Rust Core 在发布审批前要求 Main 准备一次精确绑定：

- 绑定 scope 是当前 managed surface 或 managed browser profile。
- 绑定包含 Run、activation、call、Tool、arguments digest 和过期时间。
- 若涉及上传，Main 先用原生 picker 选择文件并形成 process-only 路径能力。
- Main 冻结 exact surface generation/origin，返回 value-free target binding digest。
- Rust Core 审批后签发只适用于该参数、资源和目标的 grant；Main 调用前再次核对 opaque `targetBindingId`。

敏感风险种类包括文件读写/上传/下载、cookie、local/session storage、storage state 导入导出、敏感网络读取、页面脚本和不安全代码执行。拒绝、取消、过期、Run/capability/grant 撤销或 shutdown 都必须释放绑定及准备文件。

## 网络策略与审批

网络策略把目的地分为三类：

1. 普通公共目的地：允许。
2. 可审批风险：经 Rust Core 发布类型化审批后，在执行前重新解析和核对。
3. 不可审批的 Electron Main 边界：始终拒绝，包括主 Renderer、Electron 特权边界、内部调试端点和 MCP 控制端点。

可审批风险包括 HTTP、localhost/loopback、私网、link-local、云 metadata、非标准端口、URL userinfo、DNS 私网解析、风险升级、新窗口、上传、下载和本地服务请求。

主动自动化使用 DNS 解析、分类、HMAC target/resolution fingerprint 和最多 3 次重评估：

- 审批前解析一次。
- 审批后再次解析；身份漂移会重新审批，不能沿用旧 grant。
- 即使第一次得到公共地址，真正放行前也重新解析，降低 DNS rebinding 风险。
- 单次解析默认 2 秒，最多 16 个地址、32 个并发解析；失败/溢出拒绝。

手动浏览不要求用户为普通 HTTP/私网逐次批准，但仍执行不可绕过的 Electron Main 静态边界检查。该差异是有意设计，不能把手动导航结果复用为 Agent grant。

风险授权由 Rust Core 持有 task grant 和审批语义。Main 只对并发相同检查去重，不维护持久 grant cache。

## 调用结果与 dispatch certainty

在调用官方 handler 前，Main 必须先让 Rust Core 持久接受 `possibly_dispatched` phase。阶段为：

- `pre_dispatch`
- `possibly_dispatched`
- `response_received`

如果 phase acknowledgement 失败，调用在执行前 fail closed。执行开始后发生超时、崩溃或通道丢失时，结果必须携带 `possibly_dispatched`/`outcome_unknown`，不能谎报“未执行”。这对点击、提交、下载等有副作用 Tool 尤其重要。

## 上传、下载与 Artifact

### 文件上传

Renderer/模型不提供绝对路径。Main 用原生 picker 获取文件，按 Run/call/capability 建立短期准备记录并绑定文件 revision。Tool 完成、取消、grant 撤销或 surface 关闭时释放。

### 下载

受管 Session 的下载由 `BrowserDownloadBroker` 接管，并通过网络风险流程审批。下载内容发布为 Electron Main-owned Browser Artifact，而不是自动写入任意用户路径。

### Browser Artifact

模型和 Renderer 只看到 `BrowserArtifactReference`：opaque id、kind、display name、MIME、大小、时间、生命周期和 preview 类型。引用不含 outputDir、Target、Tool 参数、网页内容或托管路径。

协议上单个 Artifact 最大 128 MiB、最长生命周期 24 小时；图片预览最大 8 MiB，文本预览最大 256 KiB。当前 `BrowserArtifactBroker` 默认进一步收紧为单个 64 MiB、每个 Run 128 MiB、全局 256 MiB，最多 256 个 Artifact（每个 Run 最多 64 个）。预览由 Electron Main 有界读取；导出必须经过 Main save dialog，返回 path-free 的 `exported/cancelled` 结果。

## 关闭与恢复

- Browser page 关闭触发 exact surface/instance 清理，不能让重建后的同名 surface 继承旧 target binding。
- Run、Tool call 和 capability 结束时分别清理网络、上传和 Artifact 资源。
- 应用退出时先让 Core Server 关闭 Managed MCP Manager，再关闭 Main bridge 和 surfaces，最后关闭 Broker；顺序见 [Electron Host 与进程架构](../architecture/electron-host.md)。
- guest 崩溃、Target 关闭、catalog drift、timeout 和 cancel 都返回类型化安全错误；不得自动重放可能已执行的调用。

## 状态与安全不变量

1. `surfaceId`、`surfaceInstanceId`、selection revision 和 command request id 必须同时正确，才能绑定当前 guest。
2. Renderer 页面可见不等于 Agent 获得自动化权限。
3. Main 是 WebContents/CDP/文件路径权威；Rust Core 是 capability、审批和 task grant 权威。
4. 旧 surface generation、旧 DNS identity、旧参数 digest 或过期 grant 一律不可复用。
5. Electron Main hard boundary 不能通过用户审批绕过。
6. 任何可能产生副作用的失败都必须保留 dispatch certainty。
7. Renderer/模型 payload 不得含 CDP identity、HMAC、target binding id、绝对文件路径或托管 Artifact 路径。

## 代码真源

- Browser surface 协议：`packages/protocol/src/browser.ts`
- Artifact 协议：`packages/protocol/src/browserArtifacts.ts`
- Playwright/风险桥协议：`packages/protocol/src/mcp/managedPlaywrightBridge.ts`
- Renderer panel：`src/renderer/src/features/browser/BrowserPanel.tsx`
- Renderer surface bridge：`src/renderer/src/features/browser/browserSurface.ts`
- Renderer webview hook：`src/renderer/src/features/browser/useBrowserWebview.ts`
- Main webview policy：`src/main/webviews/managedWebviewSecurity.ts`
- Surface/Target：`src/main/browser/BrowserSurfaceManager.ts`、`BrowserTargetBroker.ts`
- 网络与风险：`src/main/browser/BrowserNetworkPolicy.ts`、`BrowserNetworkGuard.ts`、`BrowserRiskCoordinator.ts`
- 文件与 Artifact：`src/main/browser/BrowserFileBroker.ts`、`BrowserDownloadBroker.ts`、`BrowserArtifactBroker.ts`
- Managed MCP Host：`src/main/mcp/ManagedPlaywrightBridgeHost.ts`、`ManagedPlaywrightMcpHost.ts`
- 固定目录：`src/main/mcp/managedPlaywrightManifest.ts`、`managedPlaywrightCatalog.ts`
- 敏感绑定：`src/main/mcp/ManagedPlaywrightSensitiveTargetBindingBroker.ts`

## 测试与发布验证

关键测试：

- `packages/protocol/src/browser.test.ts`
- `packages/protocol/src/browserArtifacts.test.ts`
- `packages/protocol/src/mcp/managedPlaywrightBridge.test.ts`
- `src/main/core/managedWebviewSecurity.test.ts`
- `src/main/browser/*.test.ts`
- `src/main/mcp/ManagedPlaywrightBridgeHost.test.ts`
- `src/main/mcp/ManagedPlaywrightMcpHost.test.ts`
- `src/main/mcp/managedPlaywrightRound3Stress.test.ts`
- `src/renderer/src/features/rightSidebar/__tests__/BrowserSurface*.browser.test.tsx`
- `src/main/browser/fixtures/*.electron.ts`

除 `pnpm test:web` 外，发布相关改动应运行：

```bash
pnpm verify:playwright-round3-release
```

该命令串联固定 catalog、Round 3 stress、packaged startup 脚本测试、startup verifier 和 conformance report，但不是完整的 packaged Browser release acceptance。当前 verifier 只支持调用方提供的 macOS unpacked `.app`，不会证明该 bundle 是由当前源码新鲜构建；真实 packaged `Agent → Managed Playwright MCP → local fixture` 仍为 `pending`（`no_production_external_managed_mcp_driver`）。发布声明必须保留这两项限制，详见 [测试策略](../development/testing.md) 与 [构建和发布](../development/build-and-release.md)。

## 变更检查表

- [ ] surface command/ready/selection 的 schema、instance 和 stale 行为同步更新。
- [ ] guest 在导航前已安装 partition、webPreferences、权限和网络策略。
- [ ] 新 Tool 进入固定 catalog，69/61 数量、处理模式、参数/输出预算和 catalog drift 测试同步更新。
- [ ] 新风险被正确归类为 hard deny、网络审批或敏感 Tool 审批。
- [ ] grant 绑定 exact Run/call/arguments/resource/target/expiry，并在所有终止路径释放。
- [ ] 副作用路径的 dispatch phase 和 outcome_unknown 测试已覆盖。
- [ ] 上传不接受 Renderer 路径；下载和 Artifact 不暴露托管路径。
- [ ] 手动浏览与 Agent 自动化权限没有混用。
- [ ] 打包环境 Playwright 启动和关闭顺序已验证，且没有把 startup evidence 宣称为 packaged Agent E2E。

## 当前限制

- 内置自动化 capability 目前只有 `browser_automation`。
- 用户配置的 MCP Server 当前仍仅支持本地 stdio；Managed Playwright 是应用内置反向桥，不是通用 HTTP MCP transport。
- Browser guest 的网页权限请求全部拒绝，摄像头、麦克风、通知、地理位置等站点功能不可用。
- Browser 页面只支持 HTTP/HTTPS 和窄化的内部 PDF viewer；不支持任意自定义协议。
- Artifact 是 Run 生命周期资源，不是永久文档库；需要长期保留时必须显式导出。
- 自动化只作用于受管 BrowserContext，不连接用户系统浏览器或任意外部调试端点。
- `verify:playwright-round3-release` 仍缺真实 packaged Agent→Managed Playwright→local fixture E2E，并且 supplied bundle freshness 未建立。
