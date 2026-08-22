---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-23
---

# 威胁模型

## 范围

本文描述桌面应用的工程信任边界。它不是操作系统级沙箱证明，也不代表所有平台已经完成发布安全
认证。具体协议与限制分别见 [IPC 与协议](../architecture/ipc-and-protocol.md)、[MCP](../subsystems/mcp.md)
和[浏览器与自动化](../subsystems/browser-automation.md)。

## 受保护资产

- 用户工作区文件、附件、下载和生成制品；
- 模型、搜索、图片生成及 MCP 相关凭据；
- 对话、Trace、审批、Continuation、Agent 树和本地设置；
- 用户授权的命令、文件写入、浏览器目标和外部工具副作用；
- 应用与受管运行时的完整性、签名和冻结 receipt。

## 信任区域

```text
Untrusted web / model / MCP output / Skill content
        |
        v
Sandboxed Renderer or controlled protocol projection
        |
        v  typed Host API + trusted sender checks
Preload / Electron Main
        |
        +--> browser, terminal, filesystem and credential brokers
        |
        v  strict line-delimited JSON-RPC
Core Server -> Rust Core / SQLite / managed adapters
```

模型输出、网页内容、MCP descriptor/result、Skill 指令、附件内容和用户项目源码都应视为不可信数据。
它们不能通过“描述自己是安全的”提升权限。

## 主要边界

### Renderer 与 Host

Renderer 在 context isolation 下运行，不直接获得 Node、任意 IPC 或文件系统能力。Preload 只暴露
[`packages/host-api`](../../packages/host-api) 定义的窄接口；Main registrar 校验发送方并通过严格 parser
处理结果。新增 channel 必须同时更新 allowlist、类型、错误投影和负向测试。

### Agent 与副作用

Rust Core 生成结构化 Tool proposal，Core Server 与 Rust Core 根据当前 workspace、权限模式、Tool 风险和冻结参数决定
自动执行、请求审批或拒绝。审批必须绑定精确 action/argument/target；模型文本不能视为用户授权。

命令策略和文件路径检查不是完整 OS 沙箱。进程可能访问操作系统赋予应用的其他资源，因此高风险操作
仍需显式边界、最小权限和可审计结果。

### 浏览器

网页运行在受管 webview/session 中。默认拒绝摄像头、麦克风、定位、通知等页面权限；导航、下载、
上传、文件读取和 Agent 自动化由 Host broker。自动化能力先按任务激活，敏感目标再绑定精确 origin/风险
授权。网络策略降低 SSRF、DNS rebinding 和本地目标风险，但不应宣称等同网络沙箱。

### 外部 MCP

用户 MCP Server 是本地外部进程。保存配置、授权启动、启用连接和批准调用彼此独立。可执行文件、cwd、
argv 和已识别代码输入绑定授权身份；Server annotations 不能取消审批。参数、结果和 stderr 必须限长并
经过安全投影。调用一旦可能送出，连接丢失可能得到 `OutcomeUnknown`，禁止自动重放。

用户配置的 stdio MCP Server 进程隔离具有平台差异；Windows 进程树约束和目标平台验证仍是明确限制。

### Skills 与受管运行时

bundled、installed、workspace Skill 都是内容包，不继承额外权限。发现、激活、资源读取、脚本和恢复绑定
冻结 revision；脚本执行仍经过能力与审批。Office、Artifact、PDF 和 Managed Playwright 组件通过固定来源、版本、
receipt、schema/digest 和打包验证建立供应链边界，不能从临时缓存直接获得信任。

### 凭据与敏感 payload

不同凭据有不同持久化策略，不能统一假设都在 OS Keychain。MCP 审批和图片生成使用 Credential Store
相关边界；某些本地模型/搜索配置仍保存在 SQLite。无论存储方式如何，敏感值不得进入模型上下文、
Trace、普通日志、Renderer 事件、错误文案、命令行或测试 snapshot。

## 持久化与恢复

SQLite 是大部分领域的恢复真源。关键副作用使用 receipt、CAS、lease、checkpoint 或
`outcome_unknown` 防止崩溃后盲目重放。通知只是失效信号，不能替代持久状态。schema/catalog 不匹配时
fail closed 并要求显式开发重置，不在启动时偷偷改写旧库。

删除项目、Conversation 或 Agent 树时必须遵守领域所有权和外键规则；文件数据根中的孤儿对象只由受管
清理策略处理，不根据 Renderer 猜测直接删除。

## 已知残余风险

- 文件/命令权限是应用策略，不是通用 OS 容器；
- 外部 MCP 的路径校验到实际 spawn 之间仍存在平台相关 race；
- Windows 用户配置的 stdio MCP Server 尚不能宣称完整进程树隔离；
- macOS 已有代码签名，但 notarization 和自动更新发布链路尚未配置；
- 浏览器和运行时组件仍需要目标平台、打包态和供应链持续验证；
- 本地设备或用户账户已被攻破时，本应用不能提供可信执行环境保证。

## 安全变更检查表

- 不可信输入在每个跨进程边界是否严格解析并限长；
- 权限是否绑定精确主体、任务、目标、revision 和有效期；
- 副作用发生前后能否区分安全重试与未知结果；
- 取消、超时、崩溃和重启是否会导致重复执行或权限升级；
- 敏感值是否可能进入日志、Trace、事件、错误、Archive 或快照；
- 文件路径是否规范化、限制 symlink/根目录逃逸并由 Host 最终裁决；
- 测试是否包含拒绝、篡改、越界、重复和恢复路径；
- 文档是否明确测试未覆盖的平台和残余风险。

关键真源包括 [`trustedIpc.ts`](../../src/main/ipc/trustedIpc.ts)、
[`crates/core/src/tools`](../../crates/core/src/tools)、
[`src/main/browser`](../../src/main/browser)、
[`crates/core-server/src/application/mcp`](../../crates/core-server/src/application/mcp) 和
[`canonical_schema.sql`](../../crates/core/src/storage/canonical_schema.sql)。
