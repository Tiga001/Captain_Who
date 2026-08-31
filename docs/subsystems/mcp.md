---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-08-31
---

# MCP 子系统

本文描述当前 MCP Tool 实现、用户 stdio 与内部 HostBridge 的边界、审批和持久化安全要求。它不对尚未实现的 MCP 功能作兼容承诺。

## 1. 当前支持范围

| 能力                                                       | 当前状态                                                           |
| ---------------------------------------------------------- | ------------------------------------------------------------------ |
| 用户添加的 MCP Server                                      | 本地 stdio；显式保存、launch authorization、enable/start           |
| 内部托管 MCP                                               | Host 编译期允许的 `HostBridge`，当前用于内置浏览器自动化           |
| Tool discovery/call                                        | 支持；分页 Catalog、严格 schema/descriptor limits、调用 certainty  |
| 审批模式                                                   | `Prompt`、`Auto`、`Deny`；仍受 trust、身份、Catalog 和风险策略约束 |
| HTTP/Streamable HTTP、OAuth                                | 未实现                                                             |
| Resources、Prompts、Sampling、Elicitation、Tasks、MCP Apps | 未接入 Agent 产品路径                                              |
| 用户 env/SecretRef/headers                                 | 当前管理 UI/持久 stdio 配置不开放                                  |
| 未知结果自动重试                                           | 禁止                                                               |

“用户 MCP Server 当前仅 stdio”和“内部存在 HostBridge”必须同时成立。HostBridge 不带 executable、endpoint、credential 或用户可编辑参数；外部 Registry adapter 应拒绝用户持久化该 transport。

## 2. 分层与数据流

```text
Renderer MCP Settings / Approval / Activity
  → typed Host API + Preload allowlist
  → Main 原生路径/授权 UI
  → Core Server JSON-RPC
  → Core Server application/mcp
      ├─ SQLite Registry / policy / approval envelope
      ├─ Authorized stdio connector
      ├─ Builtin capability runtime
      └─ Managed Playwright HostBridge
  → mycopilot-mcp-client Registry / Manager / Catalog
  → stdio MCP Server 子进程或内部 HostBridge peer

Agent ToolRegistry
  → core 的 rmcp-free MCP Tool contract
  → sealed proposal / approval journal
  → Core Server `McpRuntimeBridge` 重新验证
  → McpConnectionManager.call_tool
```

### 所有权

- `crates/mcp-client`：SDK-neutral public types、协议协商、Catalog、连接生命周期、调用 certainty、limits、transport。不得依赖 Rust Core、SQLite、Electron 或 Renderer。
- `crates/core/src/tools/mcp.rs`：Agent 可见 Tool identity、provenance、审批摘要和有界模型投影，不暴露 rmcp 类型。
- `crates/core-server/src/application/mcp`：持久 Registry、launch authorization、sealed approval envelope、内置能力 policy、Browser risk 和管理用例。
- `crates/core-server/src/adapters/mcp_runtime.rs`：把 Catalog 适配到现有 ToolRegistry；不得创建第二条 Tool 执行链。
- `packages/protocol` / `crates/protocol-rs`：严格管理 DTO、managed bridge DTO 和跨语言 fixtures。
- Electron Main：受信 Browser surface、CDP、下载/文件/Artifact broker 与本地授权 UI。

MCP Server 路由身份始终是 `McpServerId + raw tool name`。模型侧的 `mcp__...` 是有界命名空间，不得反向解析成权限。

## 3. Registry、Manager 与 Catalog

`McpRegistry` 保存期望配置和稳定 model namespace；`McpConnectionManager` 订阅变更并拥有实际连接；Catalog 是已协商、已校验的可调用 Tool snapshot。

Manager 当前拆分为：

- `connection_lifecycle`：connect/start/stop/restart 和状态；
- `registry_sync`：配置 revision 与实际连接收敛；
- `catalog_refresh`：分页 tools/list、变更信号和完整性；
- `invocation`：身份重验、并发、deadline、cancellation 与 dispatch certainty；
- `shutdown`：停止 admission、取消、关闭 peer 和收割子进程。

只有 `Complete` Catalog 可以路由调用。重复 cursor、超页数/工具数、非法 schema、descriptor 超限和 normalized-name collision 会产生明确 incomplete/disabled diagnostic，不能把部分 Catalog 当成功。

连接状态和事件是进程投影；持久配置 revision、launch authorization 和 Tool action journal 才能跨重启恢复。`mcp.changed` notification 只用于刷新 UI。

## 4. 协议生命周期

stdio connector 使用 rmcp Auto 生命周期：

1. 优先尝试 `server/discover` 与协议 `2026-07-28`。
2. 仅当 discovery 被旧 MCP Server 明确判定为不支持时，回退到 `initialize` + `notifications/initialized` 生命周期与 `2025-11-25`。
3. capabilities、server metadata、现代订阅和旧 `tools/list_changed` 在 `McpPeer` 后统一。

`2026-07-28` 是当前锁定 SDK 支持的 draft/RC 路径，`2025-11-25` 是稳定 fallback。支持该路径不等于通过官方 MCP 全量 conformance。

progress notification 不持久化，也不延长 Host-owned hard deadline。取消是 best effort。一旦请求可能发出，timeout、cancel、transport close 或子进程丢失会产生 `OutcomeUnknown`；不得自动 retry/replay。JSON-RPC error response 与 `isError=true` 是已收到的权威响应，不属于 unknown。

人工审批 ticket 与短生命周期 sealed/process payload 分开。外部 MCP Server、内置敏感 Tool、Browser risk 或 Skill installation 的票据会保持 pending，直到用户决定或所属 Run 的取消/终态流程显式收口；payload TTL 不再替用户结算。若用户晚批准时精确执行材料已不可用，Host 提交一个 `dispatchCertainty=definitely_not_dispatched`、`retryable=false` 的 failed Tool Result，让原 Run/Automation continuation 正常继续；它不是 dispatch、不是 OutcomeUnknown，也不能自动重新调用。

## 5. Transport 边界

### 用户 stdio MCP Server

用户 MCP Server 配置包含 program、ordered argv、cwd、enabled/trust/approval mode 和 timeout。当前公开管理路径不接收 env/SecretRef，因此不要把凭据放入 argv 或 cwd。

stdio connector 负责：

- 有界 framing、stderr 捕获与速率限制；
- 启动前 exact launch authorization 重验；
- Unix process-group 关闭与收割；
- deadline/cancellation 与 dispatch certainty；
- 连接、崩溃、重启和 shutdown 清理。

### 内部 HostBridge

`McpTransportConfig::HostBridge` 只允许 Host 从编译期 allowlist 构造。当前 channel 为 `builtin.playwright.v1`：

- 没有网络 URL、程序路径或用户参数；
- reverse command bus 最多 8 个 pending 请求；
- request ID 是 UUIDv4；
- complete/dispatchPhase 必须 schema 与 operation 匹配，certainty 只能单调前进；
- 10 分钟 idle timeout 后可关闭 Managed Playwright runtime；
- 关停时 Core Server 与 Main 完成专门的反向 settlement。

Managed Playwright `CallTool` 的执行预算由 Electron Main 计时，以便显式 BrowserRisk 人工等待暂停但不重置剩余预算。Rust reverse bridge 对 `CallTool` 等待 completion 时不再叠加 transport deadline，但仍接受 Manager cancellation 与 shutdown；非 `CallTool` bridge operation 保留原 transport deadline。用户配置的 stdio MCP Server 仍使用 Manager-owned hard deadline，不继承该特例。

未来新增 HostBridge channel 必须有独立 manifest、Host adapter、风险策略和 E2E；不得接受任意字符串通道后动态加载。

## 6. 四类独立授权

对用户 stdio MCP Server 必须区分：

1. 保存非敏感配置。
2. 授权一次精确本地 launch specification。
3. enable/connect 该 MCP Server。
4. 批准一次冻结的 Tool invocation，或由明确 `Auto` policy 放行。

新用户 MCP Server 应以 disabled、untrusted、Prompt 起步。`Auto` 已是后端真实模式，但不是跳过安全检查的“全局永远允许”：Manager 仍重验 MCP Server/config/Catalog/Tool identity，内置敏感 Tool 仍可进入专门 Browser/Approval 策略，`Deny` 仍是最终 kill switch。UI 是否暴露某种便捷开关不能改变后端枚举事实。

Tool annotation、title、description 和 `readOnlyHint` 都来自不可信 MCP Server，只能作为展示提示，不能降低审批或文件/网络权限。

## 7. Launch authorization

当前 authorization v2 绑定：MCP Server ID、config epoch/digest、ordered argv、cwd、canonical executable/cwd，以及识别出的本地代码入口 metadata snapshot。

Unix snapshot 包含 canonical path、device/inode、ctime/mtime、size、mode 和 ownership；它不对文件内容做 hash。显式本地 import/require/loader 和受识别扩展名的代码 argv 会被 canonicalize 并进入原生确认预览，最多 16 项。

enable/start 和最终 spawn 都重新验证 snapshot。可执行文件、代码入口或 symlink 目标被替换后 authorization 失效。旧 authorization 格式恢复为 disabled/untrusted，要求重新确认。

已知残余风险：

- portable path-based spawn 仍有 validation-to-spawn race；
- Unix process group 不能约束恶意进程主动逃入新 session；
- Windows metadata snapshot 与 process-tree isolation 弱于 Unix，当前只保证直接子进程终止。

因此 launch authorization 是高风险本地确认，不是 sandbox 声明。

## 8. 持久化与秘密

Registry 只持久化显式 allowlist：MCP Server identity/display、scope/source、stdio program/argv/cwd、enabled/trust/approval、config epoch/digest、revision、model namespace、launch authorization identity 和时间。

- program、argv、cwd 是明文配置，禁止放密码、Token、Cookie。
- 原始 Tool arguments 视为一个秘密 payload。pending approval 只持久化安全结构摘要与认证加密 envelope。
- master key 应位于 OS Credential Store，不与 SQLite ciphertext 同存。安全 credential backend 不可用时只保留进程内 payload，重启后审批不能继续。
- checkpoint、trace、IPC 和 Renderer 只接收 typed identity 与有界 safe projection。
- 原始 MCP result 不做 exact archive。过大结果在 transport 拒绝；binary/image/audio/resource block 不进入普通模型上下文。
- Managed browser download 的字节由 Main broker 捕获，Core Server 通过 Rust Core storage 保存无路径 identity、hash、scope、设置与历史。模型/Renderer 只接收 `browser-download:<uuid>` 和安全元数据，不能看到受管绝对路径。

## 9. 中央安全限额

默认上限由 `McpSecurityLimits` 统一定义；transport 可更严格，不可更宽：

| 类别           |                                                      默认上限 |
| -------------- | ------------------------------------------------------------: |
| protocol frame |                                                         8 MiB |
| safe error     |                                                         4 KiB |
| stderr         |                             单行 8 KiB；保留 64 KiB；16 KiB/s |
| Catalog        |                              32 页；1,024 tools；cursor 4 KiB |
| schema         | 每 Tool 256 KiB；每页 1 MiB；总计 8 MiB；深度 32；4,096 nodes |
| descriptors    |                                       每页 2 MiB；总计 16 MiB |
| arguments      |        canonical 64 KiB；深度 32；4,096 nodes；256 properties |
| raw result     |                                     4 MiB；128 content blocks |
| encoded media  |                                    每 block 1 MiB；合计 2 MiB |
| 模型投影       |                            text 16 KiB；structured JSON 8 KiB |
| Tool timeout   |                                       默认 60 秒；最大 300 秒 |
| 并发调用       |                            每 MCP Server 32；Manager 总计 256 |

修改上限必须同时审查 allocation、wire framing、日志、持久 envelope、模型投影和压力测试，不能只改一个常量。

## 10. 内置浏览器自动化

内置能力 ID 为 `browser_automation`，Managed MCP Server ID 为 `builtin.browser_automation.mcp`。代码锁定：

- `@playwright/mcp` `0.0.79`；
- Playwright/Playwright Core `1.63.0-alpha-2026-08-05`；
- 固定 upstream Catalog 精确 69 个 Tool；reviewed manifest 当前暴露其中 61 个；
- reviewed policy 为每个 Tool 指定 pass-through、Host-adapted、approval-required、artifact-managed、sandboxed 或 unsupported；只有允许的 handling mode 可暴露给模型；
- 每个可暴露 Tool 必须含 Host `call_reason` 约束，并通过 upstream schema、overlay、Host input schema 与 digest 校验。

Main 不把任意网页或系统浏览器直接交给模型。BrowserSurface、target、network、file、download 和 Artifact 都由独立 broker 管理；敏感 target/file/action 绑定通过 `mcp.browserRisk.authorize/cancel` 与内置 pending action 流程完成。

成功下载会从 `browser_click`、`browser_get_config`、`browser_wait_for` 等结果/进度中投影 path-free reference；当前引用 contract 为 schema v2，Agent Tool Result 只接受 `source=agent`，每个结果最多 16 个，单项最多 2 GiB。后续 `read_file`、Command/Office 输入等使用该引用时，Rust Core 重新校验当前 Conversation、同一 Agent task tree、同 Project，或 read=all 下显式允许的手工下载能力。task tree scope 从持久 `agent_nodes` 推导；opaque ID、自报 root 或外部 Conversation 不能授权读取。

跨 Core Server/Main 协议当前为 Managed Playwright bridge schema v4，方法包括：

- `mcp.builtinPlaywright.command`
- `mcp.builtinPlaywright.cancel`
- `mcp.builtinPlaywright.complete`
- `mcp.builtinPlaywright.dispatchPhase`

内置 capability allow policy 通过 `mcp.builtinCapability.list` 与 `mcp.builtinCapability.setAllowed` 管理，不等同于用户 MCP Server Registry。

## 11. 代码真源

- client public contract：`crates/mcp-client/src/lib.rs`、`config.rs`、`limits.rs`
- lifecycle/stdio：`crates/mcp-client/src/connection.rs`、`transports/stdio.rs`
- Manager：`crates/mcp-client/src/manager.rs` 与 `manager/`
- Registry/persistence：`crates/core-server/src/application/mcp/sqlite_registry.rs`、`management/`
- Agent adapter：`crates/core-server/src/adapters/mcp_runtime.rs`
- Playwright manifest：`crates/core-server/src/application/mcp/playwright_manifest.rs` 及 `crates/core-server/resources/playwright-*.json`
- HostBridge：`crates/core-server/src/application/mcp/managed_playwright_bridge.rs`
- Browser Host：`src/main/mcp`、`src/main/core/browser*`、`managedPlaywrightBridge*`
- Browser download：`src/main/browser/BrowserDownloadBroker.ts`、`crates/core/src/browser_downloads.rs`、`crates/core/src/storage/service/browser_downloads.rs`
- 跨语言协议：`packages/protocol/src/mcp`、`crates/protocol-rs/src/managed_playwright_bridge.rs`

## 12. 测试

```bash
cargo test -p mycopilot-mcp-client
cargo test -p mycopilot-mcp-client --test stdio_integration
cargo test -p mycopilot-mcp-client --test stdio_integration -- --stress-suite
cargo test -p mycopilot-core-server --test mcp_stdio_runtime_e2e
pnpm test:playwright-fixed-catalog
pnpm test:playwright-round3-stress
pnpm exec vitest run --project managed-playwright-e2e
pnpm exec vitest run --project unit src/main/core/browserDownloadIpc.test.ts src/main/core/browserDownloadBroker.test.ts
pnpm test:playwright-conformance-report
```

repository-owned stdio fixture 必须确定性、离线、只启动 `current_exe()`，并断言协商、limits、cancellation、close/reap 和错误脱敏。不要使用 `npx`、用户配置或已安装 MCP Server 作为测试依赖。

官方 MCP Conformance Framework 当前没有与本项目用户 stdio client 对接的已锁定 runner/command adapter。通过 rmcp 和本地 fixture 不能表述为“通过官方全量 conformance”。

## 13. 当前限制

- 外部 MCP Server 仅 stdio；没有 Streamable HTTP、OAuth/PKCE、URL/SSRF/redirect/DNS-rebinding policy。
- 用户配置不支持 env、SecretRef、headers；类型中的 env binding 是下层扩展边界，不代表产品已开放。
- Resources/Prompts/Sampling/Elicitation/Tasks/Apps 未进入产品路径。
- Windows stdio 当前不能声明 process-tree isolation 或 release acceptance。
- Manager lifecycle 使用进程内 channel；通知风暴虽会 coalesce，未来更高流量仍需 bounded/lag-aware queue。
- Managed Playwright 尚无“61 个模型可见 Tool × 成功/非法输入/取消/超限/target-close”的完整自动矩阵；69 是 upstream 总数，不能与 exposed 数混用。
- Browser download 的同树/同 Project 复用是本地授权便利，不是跨树共享；当前没有跨设备同步或成员级转授权。
- Packaged Managed Playwright gate 目前只证明指定的 unpacked macOS bundle 中应用/Core Server/Renderer 与锁定依赖可以启动；真实 packaged Agent → Managed MCP Server → 本地 fixture 仍为 pending，详见构建发布文档。

## 14. 变更检查表

- [ ] 新 MCP Server/transport 是否明确属于用户 stdio、内部 HostBridge 或新的独立安全模型？
- [ ] Registry、Manager、Catalog 与 Agent adapter 是否仍保持 transport-neutral？
- [ ] 协商版本、capability 和 list_changed 是否在真实 wire fixture 中覆盖？
- [ ] config/launch/tool/risk 四类授权是否没有被合并或绕过？
- [ ] 新参数/结果是否有 byte/depth/count 上限、redaction 和持久化策略？
- [ ] timeout/cancel/close 是否定义 dispatch certainty 和 `OutcomeUnknown`？
- [ ] 人工 ticket 与 sealed/process payload TTL 是否分离，晚批准缺少材料时只生成 definitely-not-dispatched 失败？
- [ ] 新 HostBridge 是否是编译期 allowlist，且有严格 schema、pending 上限、idle/shutdown？
- [ ] Managed `CallTool` timeout 是否仍由 Main 计时、只暂停显式人工等待，并保留 cancellation/shutdown？
- [ ] 下载是否只投影 path-free reference，并在后续消费时重新校验 Conversation/tree/Project scope？
- [ ] 更新 Playwright 包或 Tool 时，是否同步 upstream Catalog、reviewed manifest、digests、package verifier 和 conformance report？
- [ ] 是否运行 stdio、Core Server E2E、Main/Renderer、固定 Catalog、stress 和打包相关测试？
- [ ] 是否更新本文的支持矩阵与当前限制，且未扩大 conformance 声明？
