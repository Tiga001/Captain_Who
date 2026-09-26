---
status: current
audience: developers
owner: engineering
last_verified: 2026-09-26
---

# 命令运行时与 Command Session

本文描述 `run_command` 的策略、执行、输出捕获、长任务移交和持久 Session 恢复。Command Session 是一个已授权 OS 进程的受管生命周期，不是通用终端，也不提供任意 stdin。通用审批边界见[Tool 体系、权限与审批](./tools-permissions-and-approvals.md)，输出预算见[Tool Result 上限、分页与恢复](./tool-result-limits.md)。

## 职责边界

- Command parser/policy 将模型文本规范化、分段、分类，并决定 allow/approval/deny。
- Core Server 把需要审批的命令冻结为 `AgentCommandRequest`，持久化批准和 audit 后才启动。
- Rust Core 的 `CommandSessionManager` 拥有 OS process group、输出读取、控制信号和内存中的权威状态机。
- Core Server command-session service 把 lifecycle、transcript、model read receipt、published output 和恢复状态写入 SQLite。
- `command_session` Tool 只 wait 或 interrupt 已存在 Session；它不能启动命令、修改命令或发送 stdin。
- Artifact Runtime/Office/managed builder 负责可复现运行环境和产物后处理，不改变命令授权来源。

## 命令准备与策略

```text
model command + cwd + inputs + expectedOutputs + timeout
  -> normalize CRLF/newlines and reject NUL/background/unsafe heredoc forms
  -> lexical segmentation of newline | pipeline | && | || | ;
  -> canonical cwd/workspace/input mounts
  -> classify every segment
  -> combine risk and AgentPermissions
  -> deny | require explicit approval | automatic authorization
  -> freeze canonical AgentCommandRequest and runtime profile
```

风险类：`ReadOnly`、`SafeWorkspaceWrite`、`DirectWrite`、`Network`、`PackageManagement`、`HighImpact`、`ExternalRead`、`Unknown`、`Catastrophic`、`Unsupported`。Guarded 自动路径只允许策略认可的低风险命令；显式批准可放行部分高风险命令，但 `Catastrophic`/`Unsupported` 仍拒绝。`FullAccess` 也不绕过 always-denied、语法、scope、timeout 和路径检查。

`CommandAuthorizationSource::Automatic|ExplicitUser` 是 Core Server 持有的授权事实。批准后执行必须重新验证 frozen command、cwd、inputs、policy revision 和 runtime fingerprint，不能直接执行原始模型字符串。

当前 command 最大 16,000 字符，timeout 最多 600 秒；`timeout=None` 对普通 session 表示没有进程硬截止时间，初始 yield 不是 timeout。

## 输入与运行环境

命令默认在已验证 workspace/cwd 中运行。Attachment、Skill Resource、Generic Artifact 和 durable Browser Download 输入先解析为不可变 `AgentFileInputRef`，再放入私有只读 input root；模型可见 URI/alias/`browser-download:<uuid>` 不直接传为宿主路径。当前最多 16 个输入；普通文件使用流式摘要与复制，不再受统一单项 64 MiB、合计 128 MiB 的旧 mount 上限约束。输入来源、视觉处理和解析器仍各有独立限制，真源见 [`file_input.rs`](../../crates/core/src/file_input.rs)。

多目录项目以当前 Run 的冻结 workspace membership 解析工作目录和文件输入；项目随后增删目录不会扩展本轮
命令授权。Composer 文件夹引用只授予读取，不因展示了绝对路径就授予写入/命令范围。已导入附件通过持久引用
进入附件库后再物化，不能把未完成的 import 或 Renderer 提交的路径当作可执行输入。详见
[工作区与文件](workspace-files.md)及[会话输入](conversation-inputs.md)。

Browser Download materialization 对 Agent 下载先验证当前 conversation/project 或受信 Agent task tree authority；手动下载仅在当前输入策略明确允许 manual download 时接受。随后重新核对 durable record 的 size/hash/文件身份；missing、modified、越权或已替换文件 fail closed。其宿主下载路径只在 Rust Core/Electron Main 私有边界存在，命令看到的是受限 mount。未知 virtual resource prefix 不能回退为普通文件路径。

Runtime Profile 可以要求 packaged Artifact Runtime 中的 Node、Python、ripgrep 和固定依赖。Discovery 校验 component receipt、文件 hash、版本和 runtime fingerprint。运行时发现只证明可用性和完整性，不授予执行权限。

受管 PDF 文本提取会在尺寸预算完成后移除 U+0000，再交给 ripgrep/文本消费者，避免合法提取结果被误判为二进制；该清洗不改变原始文件、hash 或预算计量，也不意味着任意 binary output 可当文本处理。

受管 PDF、Office builder 和 presentation editor 使用额外的命令形状、语法预检、环境变量、输出路径和超时。不得通过普通 `run_command` 参数伪造 managed profile。

## 启动与移交

`run_command` 启动进程后先等待一个有界 initial yield（默认 10 秒，可归一到 10 ms–30 秒）：

- 进程在窗口内退出：直接返回 terminal result；
- 仍运行：返回 `status=running`、`sessionId=cmd_<32 hex>` 和当前输出预览，Run 可继续。

Session scope 绑定 Core Server 的 conversation/Run scope。Manager 当前默认全局最多 32 个活跃 Session、每 scope 8 个，保留 256 个内存终态；这些是进程安全限，持久 repository 另有自己的保留/压缩规则。

GUI/长期服务返回 running 后，模型通常应继续任务，不反复 wait。构建/测试结果确实是下一步依据时，调用一次 `command_session action=wait`；Core Server 默认可静默等待 120 秒，最大控制等待 300 秒。普通输出不会自行触发新的模型 Turn。

## Session 状态机

```text
Starting
  -> Running
      -> Exited { exit_code }
      -> Interrupted
      -> TimedOut
      -> Failed

持久 Core Server 恢复还可能投影 OutcomeUnknown
```

`Started -> Output(sequence)* -> Terminal` 是唯一 lifecycle 序列。stdout/stderr 共享一个严格递增 sequence 域；poll、持久 transcript、Renderer 和 model receipt 不能各自发明 cursor。Terminal 只有在 OS 结算、输出 drain、completion hook（Artifact 发布等）以及 lifecycle callback 返回后才可对等待者可见。

协议/数据库将 `OutcomeUnknown` 序列化为 `outcome_unknown`。它是 Session 跟踪的终态：系统无法证明进程最终结果，不能声称仍在后台运行，也不能声称成功。

## 输出、读取回执与 Exact Archive

- 进程 stdout/stderr 各提供最多 128 KiB 的即时预览，共享 64 MiB Exact Capture spool。
- 内存 transcript 默认 256 KiB，采用 bounded head/tail chunk 策略；持久 transcript 最多 2,048 chunks。
- `command_session` 每次给模型最多 4 KiB 新输出，并只在成功提交 model read receipt 后推进模型 cursor。
- Tool 调用取消时不得消费 read receipt；重试可重新看到同一未确认输出。
- 终态 spool 只归档一次。`command_session` 轮询结果设置 `archives_result=false`，避免递归复制。

持久 repository 每个 conversation 最多保留 128 个已解决终态 Session、每个 Session 最多保留 64 个 model read receipts；`outcome_unknown` 因仍代表未解决的不确定性而固定保留，不参与普通终态裁剪。

预览的 `stdoutPreviewTruncated`/`stderrPreviewTruncated` 不等同于来源截断。只有 capture 达 64 MiB、读取失败等才设置 `truncatedAtSource` 和 stop reason。

## Wait 与 interrupt

`command_session wait` 观察当前状态和自上次模型回执以来的输出，在达到 Core Server wait deadline 时可返回仍 running。它不延长命令 hard timeout，也不自动再次调用模型。

`interrupt` 使用 sticky intent：同一个 Tool 调用因持久化重试而重放时，不会重复发送 SIGINT。Rust Core 先请求温和中断，经过 grace 后可强制终止整个 process group；同时设置 completion cancellation，阻止退出后的受管 Artifact/Office 发布事务继续提交。

Tool 声明 authoritative cancellation：Run 取消时 Core Server 返回有界权威结果，不让 detached worker 在取消后推进 cursor 或发送控制信号。

## 受管输出与 Artifact observation

执行前可对 workspace/expected output 建立有界快照，终态后计算 changes、hash、类型和完整性。Observer 有目录项、文件数、单文件大小和时间预算；`partial`/`stopReasons` 明确观察边界。

受支持的 PNG/JPEG/WebP/PDF 可在 completion hook 中发布为内容寻址 Artifact，并记录 session published output。发布失败不能改写进程 exit code，但必须进入结构化终态；若文件可能已发布而数据库 commit 不确定，则保守返回 commit unknown。

## 持久化与启动恢复

Core Server 持久化 session identity、state、projection、输出 chunks、最新 sequence、model read receipt、terminal execution payload、published outputs 和 lifecycle delivery。关键写入使用幂等 identity/CAS。

重启时：

- 已持久终态直接重建 terminal projection；
- 进程不可能跨应用进程继续受管；启动 reconciliation 将遗留 `starting`/`running` 统一结算为 `outcome_unknown`，不会伪造 `interrupted` 或假定仍运行；
- 未确认的 model read receipt 不推进 cursor；
- terminal payload 有压缩/解码大小限并重新校验 schema；
- incomplete output/publication 采用保守诊断，不自动重启命令。

## 不变量

1. 命令必须先解析所有 segment 再授权，不能只按首个 executable 分类。
2. 用户批准绑定 frozen canonical request；模型字符串不直接成为执行 authority。
3. 一个 session 只有一个 process group、一个 transcript sequence 和一个 terminal result。
4. wait 不消费未成功提交的 model receipt，interrupt 重试不重复发信号。
5. initial yield、wait deadline 和 process hard timeout 是三个不同概念。
6. terminal Archive 只写一次；轮询不递归归档。
7. unknown outcome 禁止自动重启或声称成功。
8. Browser Download 等 virtual input 每次 materialize 都重新校验 owner 与内容 identity，绝对来源路径不进入模型、Trace 或 Renderer。

## 代码真源

- Parser/policy：`crates/core/src/command/lexer.rs`、`segment.rs`、`risk.rs`、`policy.rs`
- Spawn/execution：`command/spawn_plan.rs`、`execution.rs`、`process_control.rs`
- Capture/transcript：`command/output_capture.rs`、`transcript.rs`
- Session：`command/session.rs`、`session_manager.rs`
- Managed runtime：`command/managed_runtime.rs`、`runtime_profile.rs`
- Artifact observation：`command/artifact_observer.rs`、`managed_output_publication.rs`
- File input/resource locator：`crates/core/src/file_input.rs`、`crates/core/src/resource_locator.rs`
- Tool：`crates/core/src/tools/run_command.rs`、`command_session.rs`
- 持久 repository：`crates/core/src/storage/agent_command_session_repository.rs`
- Core Server lifecycle：`crates/core-server/src/application/agent/command_sessions.rs`

## 测试

- `crates/core/src/command/tests/policy_basics.rs`
- `crates/core/src/command/tests/policy_complex.rs`
- `crates/core/src/command/tests/session_manager.rs`
- `crates/core/src/command/managed_runtime/tests/`
- `crates/core/src/tools/run_command/tests/`
- `crates/core/src/storage/agent_command_session_repository/tests.rs`
- `crates/core-server/src/application/agent/tests/command_sessions.rs`
- `crates/core-server/src/application/agent/tests/managed_command_loop.rs`
- `crates/core-server/src/application/agent/tests/terminal_events.rs`

## 变更检查表

- [ ] 新 shell 语法在 lexer/segment 和风险组合中 fail closed。
- [ ] allowlist/risk 变更覆盖 automatic 与 explicit 两个授权来源及跨平台测试。
- [ ] frozen request 包含 cwd、inputs、timeout、runtime/expected-output identity。
- [ ] Browser Download/其他 virtual input 覆盖 owner、task-tree、missing/modified、hash、未知 prefix 和 path-free projection。
- [ ] 新 Session 状态更新内存、protocol、repository、Renderer 和 startup recovery。
- [ ] output sequence、head/tail retention、model receipt 在并发/崩溃下不跳项。
- [ ] cancellation/interrupt/force-kill 覆盖子进程组和 completion hook。
- [ ] 新 managed output 通过验证、内容寻址发布并覆盖 unknown commit。
- [ ] 同步工具消费者与限制文档。

## 当前限制

- 不支持任意 stdin 或交互式 TTY；需要交互的程序应使用非交互参数或专用工具。
- 应用重启后不能重新附着旧 OS 进程，只能依据持久证据保守结算。
- Command policy 是受审计的常用命令模型，不是完整 shell 语义证明器；未知结构走审批或拒绝。
- 长日志最终受 64 MiB capture、transcript chunk 和保留预算约束。
- 平台信号和 process-group 行为存在 Unix/Windows 差异，必须保持平台专项测试。
