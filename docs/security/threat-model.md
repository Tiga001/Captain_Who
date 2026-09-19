---
status: current
audience: developers
owner: engineering
last_verified: 2026-09-16
---

# 威胁模型

## 范围

本文描述桌面应用的工程信任边界。它不是操作系统级沙箱证明，也不代表所有平台已经完成发布安全
认证。具体协议与限制分别见 [IPC 与协议](../architecture/ipc-and-protocol.md)、[MCP](../subsystems/mcp.md)、
[浏览器与自动化](../subsystems/browser-automation.md)和 [Scheduled Automation](../subsystems/scheduled-automations.md)。

## 受保护资产

- 用户工作区文件、FileChange 草稿/审计、附件、下载和生成制品；
- 模型、搜索、图片生成及 MCP 相关凭据；
- 对话、Trace、审批、Continuation、Agent 树和本地设置；
- 普通根任务与 Scheduled Automation 的通知 event/batch，以及 Automation 的 prompt、schedule、目标、冻结权限、Run 与 attention；
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

### Renderer 与 Electron Host

Renderer 在 context isolation 下运行，不直接获得 Node、任意 IPC 或文件系统能力。Preload 只暴露
[`packages/host-api`](../../packages/host-api) 定义的窄接口；Main registrar 校验发送方并通过严格 parser
处理结果。新增 channel 必须同时更新 allowlist、类型、错误投影和负向测试。

### Agent 与副作用

Rust Core 生成结构化 Tool proposal，Core Server 与 Rust Core 根据当前 workspace、权限模式、Tool 风险和冻结参数决定
自动执行、请求审批或拒绝。审批必须绑定精确 action/argument/target；模型文本不能视为用户授权。

普通文本/代码的唯一模型可见专用结构化写入入口是 `apply_patch`。Direct update/delete 必须绑定同一 Run 对精确目标的
未消费 `read_file` Observation；create 由 Host 私下证明 missing，并以 no-clobber 发布。Staged 内容只保存在
Host 私有 SQLite 行，Trace 和普通 Renderer event 只持有 body-free digest；审批/历史 diff 从冻结
FileChange/audit 快照按需读取。Run grant 又绑定 Run、Conversation/Project、scope、目录 identity、批准 action
与 permission/tool/provider revisions。symlink、hard link、special file、父目录交换和 observation/receipt 不一致
都会 fail closed；`outcome_unknown` 要求检查权威文件，不能盲重放。

命令策略和文件路径检查仍不是完整 OS 沙箱。进程可能访问操作系统赋予应用的其他资源，因此高风险操作
仍需显式边界、最小权限和可审计结果。`run_command` 中的 shell 重定向等操作可能产生独立文件副作用，
但必须由 Command policy 识别、审批和审计，不能被描述为受 FileChange Observation/audit 契约保护；旧 writer
alias 或手工修改 SQLite 也不能作为绕过正式副作用边界的入口。

### Scheduled Automation 与无人值守权限

Scheduled Automation task 是对未来 Turn 的持续授权，不是普通提醒。只有应用与 Core Server 运行时才会执行，但用户不一定在屏幕前。创建或更新 task 时，Core Server 解析权限模式并把精确权限 projection 冻结到配置；每个 Run 又保存配置 snapshot。admission 时当前设置只作为 revocation ceiling：Full/Custom 模式已被关闭就 fail closed 并将 task 标为 `permission_disabled`，不会回退到 Default，也不会自动扩大权限。

当前三个模式的执行边界是：

| 模式    | 主要权限语义                                                                                    |
| ------- | ----------------------------------------------------------------------------------------------- |
| Default | workspace 内读写；command、patch 与 built-in execution 需要 Approval；command safety 为 guarded |
| Full    | 全范围读写；command、patch 与 built-in execution 自动批准；command safety 为 full access        |
| Custom  | 冻结用户保存时的 read/write/command/patch/builtinExecution；command safety 强制为 guarded       |

`builtinExecution=auto_approve` 只跳过受信内置 Skill 脚本和插件/内置能力工具的点击；不会扩大文件/命令范围，
也不会跳过 manifest、source/revision/digest、路径、动态风险或运行时边界。Automation 保存 Custom 快照时同样冻结
这一维度，admission 仍受当前模式 enabled 状态限制。

修改 Custom 设置的具体字段不会自动收窄已经保存的 task snapshot；只要 Custom 仍启用，旧 task 会继续使用原冻结权限。用户要降低权限时必须同时更新相应 task，或关闭该模式使未来 admission fail closed。暂停 task 只阻止未来调度，不会取消已 `running`/`waiting_for_approval` 的 Run；当前 Scheduled UI 也没有独立的 active Run cancel 操作。因此 Full 和含自动批准能力的 Custom 都具有明确的无人值守副作用风险。

Approval authority 仍是原 Agent pending action。通知、attention、task prompt 或模型说明都不能批准动作；Default/受限 Custom Run 可以持久停在 `waiting_for_approval`，重启后只能对同一 `agentRunId + actionId` 批准或拒绝。

### 浏览器

网页运行在受管 webview/session 中。默认拒绝摄像头、麦克风、定位、通知等页面权限；导航、加载错误、guest
crash、下载、上传、文件读取和 Agent 自动化由 Electron Main broker。Renderer 与模型只接收 path-free download
identity/projection；本机目标路径仅保留在 Host/Core Server 边界，显式 reveal/copy-path 也由 Main 执行而不通过 IPC
返回路径。
“每次询问保存位置”只适用于手动下载，Agent 下载不会触发原生 save dialog。自动化能力先按任务激活，敏感
目标再绑定精确 origin/风险授权。网络策略降低 SSRF、DNS rebinding 和本地目标风险，但不应宣称等同网络沙箱。

### 外部 MCP

用户 MCP Server 是本地外部进程。保存配置、授权启动、启用连接和批准调用彼此独立。可执行文件、cwd、
argv 和已识别代码输入绑定授权身份；Server annotations 不能取消审批。参数、结果和 stderr 必须限长并
经过安全投影。调用一旦可能送出，连接丢失可能得到 `OutcomeUnknown`，禁止自动重放。

用户配置的 stdio MCP Server 进程隔离具有平台差异；Windows 进程树约束和目标平台验证仍是明确限制。

### Skills 与受管运行时

bundled、installed、workspace Skill 都是内容包，不继承额外权限。发现、激活、资源读取、脚本和恢复绑定
冻结 revision；脚本执行仍经过能力与审批。Office、Artifact、PDF 和 Managed Playwright 组件通过固定来源、版本、
receipt、schema/digest 和打包验证建立供应链边界，不能从临时缓存直接获得信任。macOS signer 只签 frozen
receipt 枚举的精确 Mach-O 集，并只给 Chromium/OfficeCLI/managed Node 等必需目标 `allow-jit`；afterPack 还会
扫描 app/ASAR 中的敏感状态文件、绝对 symlink、builder 私有路径、高置信 secret 和 credentialed URL。

### 凭据与敏感 payload

模型、搜索和图片生成 secret 与 SQLite 配置分离：SQLite 只保存不透明 reference、状态与非秘密元数据。
具备稳定签名身份的发行构建使用操作系统凭据存储；未签名 macOS 开发构建使用权限收紧的私有文件
backend。Renderer 只收到状态和用户本次新输入的替换值，不收到旧 secret 或 reference。凭据解析不得在
SQLite transaction/mutex 内进行；runtime 必须按所选连接的冻结 revision 解析，不能让一次不相关的失效
reference 阻断全部 Provider catalog。无论存储方式如何，敏感值不得进入模型上下文、Trace、普通日志、
Renderer 事件、错误文案、命令行或测试 snapshot。

当前 schema 的 SQLite backup 不含当前模型/搜索 secret，但旧 schema backup 可能仍有明文。只恢复
SQLite 不会恢复操作系统凭据；应用层清除也不承诺对 SSD、系统备份或系统凭据后端安全擦除。

### 原生通知

Rust Core 所有的 notification event/batch 同时接收 `human_root` 与 `automation` 事实，并持久保存有界 subject/presentation
字段。Electron Main 以短 lease 消费，在原生显示前向 Core Server 做最终语义校验，只有 Electron `show` 事件后
才 ACK。已读/删除任务、blocked 修复或 Approval settlement 后的 stale delivery 会被 suppress。全局禁用由 Rust Core
直接 suppress；应用前台或平台不支持时由 Main 终态 suppress，而不是无限保留待发送。原生通知不是授权边界，
也不是可靠告警通道；当前没有 Renderer toast fallback。

通知内容可能显示在操作系统通知中心或锁屏，`showTaskContent` 只能控制有界任务摘要，不能承载 secret、完整
Tool payload 或敏感模型输出。进程在 `show` 与 ACK 之间崩溃时，lease 恢复可能导致一次重复通知；Electron 39
缺少稳定 replacement ID，普通 batch 增量不会弹第二张 toast，只有 attention priority 升级可替换一次。若
Renderer 尚未 ready，Main 只以 FIFO 保留最多 32 个 pending open request，溢出时丢弃最早请求。这些限制
不得通过自动批准、无界扩大重试或跳过最终校验来规避。

## 持久化与恢复

SQLite 是大部分领域的恢复真源。关键副作用使用 receipt、CAS、lease、checkpoint、FileChange delete journal
或 `outcome_unknown` 防止崩溃后盲目重放。通知只是失效信号，不能替代持久状态。schema/catalog 不匹配时
fail closed。唯一支持的 exact v47 → v48 升级只补建派生搜索索引，事务失败完整回滚并保留源库；其余旧版要求显式开发重置。正式开发 reset 只从 exact current v48 及 exact v35–v47 提取
明确 allowlist；旧 schema 不保留永久兼容读取路径，绑定 exact v33 fingerprint 的私有备份恢复是受控例外。notification facts、Browser history/download records、
Agent templates 和 FileChange 运行/审计状态不会迁移。

删除项目、Conversation 或 Agent 树时必须遵守领域所有权和外键规则；文件数据根中的孤儿对象只由受管
清理策略处理，不根据 Renderer 猜测直接删除。

## 已知残余风险

- 文件/命令权限是应用策略，不是通用 OS 容器；
- 外部 MCP 的路径校验到实际 spawn 之间仍存在平台相关 race；
- Windows 用户配置的 stdio MCP Server 尚不能宣称完整进程树隔离；
- macOS 已有 app/managed native target 与 DMG signing，但仓库没有独立 DMG verifier，notarization、stapling 和自动更新发布链路尚未配置；
- packaged privacy scan 当前只在 macOS afterPack 执行，非 macOS 不能继承该证据；
- 浏览器和运行时组件仍需要目标平台、打包态和供应链持续验证；
- Full/Custom Automation 的冻结权限可在无人值守时产生副作用；Custom 细项变更不会自动收窄旧 task，暂停也不取消 active Run；
- 普通任务/Automation 原生通知可能被前台/disabled/平台策略抑制、延迟或在 show/ACK 崩溃窗口重复，且通知内容受操作系统锁屏展示策略影响；
- FileChange 只覆盖单目标普通 UTF-8 文本且最多 4 MiB；命令与专用 Builder/Office 输出仍有独立副作用边界；
- Scheduled Automation 尚无真实时钟、Provider/Approval、OS 通知点击、休眠唤醒或 packaged Electron 自动化 E2E；
- 本地设备或用户账户已被攻破时，本应用不能提供可信执行环境保证。

## 安全变更检查表

- 不可信输入在每个跨进程边界是否严格解析并限长；
- 权限是否绑定精确主体、任务、目标、revision 和有效期；
- 副作用发生前后能否区分安全重试与未知结果；
- 取消、超时、崩溃和重启是否会导致重复执行或权限升级；
- Scheduled Automation 是否冻结精确权限、在 admission 重检 revocation，并明确 pause 对 active Run 无效；
- `builtinExecution` 是否只跳过点击，仍保留 manifest/revision/digest、风险和独立文件/命令权限检查；
- 原生通知是否保持 final validation、ACK-after-show，且没有被当作 Approval 或可靠交付证明；
- 敏感值是否可能进入日志、Trace、事件、错误、Archive 或快照；
- FileChange 是否绑定 Observation、Run grant、目录 identity、audit/receipt，限制 symlink/hard link/根目录逃逸，并对 unknown outcome 禁止盲重放；
- Browser download 是否只向 Renderer/模型暴露 path-free identity，并保持手动/Agent save-dialog 边界；
- packaged 组件是否通过 frozen receipt/Mach-O allowlist、最小 entitlement、privacy scan 与最终产物验签；
- 测试是否包含拒绝、篡改、越界、重复和恢复路径；
- 文档是否明确测试未覆盖的平台和残余风险。

关键真源包括 [`trustedIpc.ts`](../../src/main/ipc/trustedIpc.ts)、
[`crates/core/src/tools`](../../crates/core/src/tools)、
[`crates/core/src/file_change`](../../crates/core/src/file_change)、
[`crates/core/src/storage/file_change_run_grant_repository.rs`](../../crates/core/src/storage/file_change_run_grant_repository.rs)、
[`src/main/browser`](../../src/main/browser)、
[`crates/core-server/src/application/automation`](../../crates/core-server/src/application/automation)、
[`src/main/notifications/systemNotificationCoordinator.ts`](../../src/main/notifications/systemNotificationCoordinator.ts)、
[`crates/core/src/storage/notification_repository.rs`](../../crates/core/src/storage/notification_repository.rs)、
[`crates/core-server/src/application/mcp`](../../crates/core-server/src/application/mcp) 和
[`canonical_schema.sql`](../../crates/core/src/storage/canonical_schema.sql)。打包安全真源为
[`verify-packaged-privacy.mjs`](../../scripts/verify-packaged-privacy.mjs)、
[`frozen-macho-signing.mjs`](../../scripts/frozen-macho-signing.mjs) 与
[`sign-macos.mjs`](../../scripts/sign-macos.mjs)。
