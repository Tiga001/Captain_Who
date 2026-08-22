---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-23
---

# Tool 体系、权限与审批

本文说明 Agent Tool 如何注册、暴露、授权、审批、执行、投影和恢复。具体 Tool 输出字段见[Tool Result 消费者矩阵](./tool-result-consumer-matrix.md)，大小限制见[Tool Result 上限、分页与恢复](./tool-result-limits.md)。

## 职责边界

- 模型可以选择已暴露的 Tool 并提供参数，但不能授予文件、命令、网络、MCP Server、Capability 或 Skill 权限。
- `ToolRegistry` 拥有 Tool 身份、schema、暴露方式、权限域、审批准备、执行入口、取消结算和消费者投影。
- Runtime 冻结本次请求的 `EffectiveToolSet`，执行无需 Core Server 副作用的 Tool，并在审批点生成 Checkpoint。
- Core Server 持久化 pending action/audit，处理用户批准或拒绝，重新验证冻结动作，执行 Core Server-owned 副作用并提交终态。
- Renderer 只展示 Event 投影并提交用户决定，不持有执行凭据。

## 注册与暴露

每个 `AgentTool` 必须实现 `definition`、`permission_policy` 和 `exposure`；按需覆盖异步执行、审批准备、取消结算及六类消费者投影。

暴露方式：

| 类型                 | 含义                                                                     |
| -------------------- | ------------------------------------------------------------------------ |
| `Stable`             | 权限允许时属于稳定 Tool 前缀；定义排序和 revision 是 Provider cache 契约 |
| `Dynamic`            | 由当前运行的扩展/目录动态加入，单独排序并参与 dynamic revision           |
| `RequiresCapability` | 只有经验证的 Skill 或内置 Capability 激活后才进入动态 Toolset            |

`EffectiveToolSet` 同时冻结 stable/dynamic definition、Tool typed identity、active capability 与整体 revision。模型只能调用该请求实际暴露的名称；“Registry 中有实现”不等于“本次请求已授权”。Capability ID 由后端从已验证 manifest 推导，不能接受模型自报 ID。

当前注册面可按以下族理解：

- 文件与检索：attachments、`read_*`、workspace/search、web、Git；
- 写入与执行：`apply_patch`、`write_file`、`run_command`、`command_session`；
- Office 与图像：三个 Office Tool、`image_generation`；
- Skills：resource list/read/materialize、script preflight/run、install prepare/commit，以及运行扩展 `skills_activate`；
- 历史与协作：`conversation_history`、spawn/send/followup/wait/list/interrupt；
- 内置能力：`activate_capability` 和激活后的 Managed Playwright Browser Tool；
- 外部扩展：MCP Server Tool 与其他 Runtime Extension。

基础 Registry 总是注册文件/搜索/Git/写入/命令、Command Session 及 Skill Resource/Script Tool；Office、图像生成、Web Tool 仅在对应 engine/execution/API key 可用时注册。历史、Skill 安装、协作、Runtime Extension、Managed Playwright 和外部 MCP Server Tool 由 Core Server 在构造 Run 时追加。上述族列表不是可执行 allowlist；完整真源是 `ToolRegistry` 的注册调用、`EffectiveToolSet` 契约和相关测试，新增 Tool 必须让自动化检查发现，而不是只修改本文。

## 权限模型

`AgentPermissions` 是后端权威值：

| 维度           | 当前值                              | 说明                                                              |
| -------------- | ----------------------------------- | ----------------------------------------------------------------- |
| read           | `workspace_only` / `all`            | 工作区、附件与受管引用；`all` 才允许受支持的外部绝对路径/系统别名 |
| write          | `denied` / `workspace_only` / `all` | 控制所有 file-write 域 Tool；可见性不代表写权限                   |
| command        | `require_approval` / `auto_approve` | 用户正常审批偏好                                                  |
| command safety | `guarded` / `full_access`           | 独立的风险上限；always-denied 操作不因 full access 放行           |
| patch          | `require_approval` / `auto_approve` | 结构化文件/Office 写入是否弹窗，仍保留路径与 revision 校验        |

模板、项目、父 Agent 和动态 policy 之间使用逐维 `meet`，只能收紧不能扩权。Tool 用 `AgentToolPermissionPolicy::Default` 或 `FileWrite(ReadWrite|WriteOnly)` 声明权限域；禁止在 Runtime 中维护第二份按 Tool 名判断的易漂移 allowlist。

## 文件与 URI 授权

模型路径先进入 `file_input`/filesystem router：

- workspace 相对路径；
- 已注册 attachment 路径；
- `skill://<package>/<revision>/<resource>`；
- `artifact://sha256/...` 或 `image-artifact://sha256/...`；
- 只有 read/write=`all` 时允许的受支持系统别名和外部绝对路径。

解析过程执行规范化、父目录与 symlink/reparse 检查、conversation grant 校验、文件 identity/revision 校验，并为命令/Office 创建私有只读输入 mount。当前一次文件输入最多 16 项，单项 64 MiB、合计 128 MiB；视觉输入单项上限为 8 MiB。URI 是稳定引用，不是裸本地路径，不能被字符串替换绕过授权。

写入继续执行：目标 scope、无 symlink 父链、基础 revision、创建/覆盖模式和原子发布检查。`write_file` 使用受管 draft，当前单 draft 4 MiB；`apply_patch` Core Server 层 patch 上限 512 KiB，并拒绝不支持的 Office/PDF 二进制修改。AutoApprove 只跳过用户点击，不跳过这些检查。

## 审批状态机

```text
model ToolCall
  -> registry validates schema / identity / effective toolset
  -> prepare typed AgentProposedAction
      -> normalize paths/argv/provider identity
      -> freeze revisions, hashes, call/Run/conversation binding
      -> acquire temporary prepared resources when needed
  -> policy decision
      -> direct execution
      -> auto-execute prepared action
      -> durable pending action + audit + checkpoint
  -> user decision (CAS)
      -> rejected: terminal rejection + release prepared resources
      -> approved: mark authority before side effect
  -> Core Server revalidates frozen payload and current authority
  -> execute exactly once
  -> persist canonical result, projections, audit and resumed Run
```

当前 typed proposed action 包括普通 ToolCall、外部 MCP Server Tool、内置 Capability 激活、内置 MCP 敏感调用、Browser risk、Diff、FileWrite、Command、Skill materialization/script/installation 和 Office operation。

“无需弹窗”不一定等于“直接执行”。自动 MCP Server 调用或需要冻结资源的动作仍必须先 prepare，以便 Core Server 获得一次性的权威 payload、TOCTOU 校验和审计身份。

## MCP Server 与内置 Capability

- 外部 MCP Server Tool 使用 typed MCP identity；模型可见名不能用于判断来源。其原始参数由受限授权信封处理，普通 Checkpoint 禁止直接持久未知/外部 MCP 参数。
- `activate_capability` 的 UI 语义是当前任务批准；实际 live grant 绑定当前 Run、manifest digest 与 policy revision，只存在于 Core Server 进程内存。新 Run 仍需重新批准，Checkpoint 不序列化该 grant。激活后 Tool 进入 dynamic Toolset 并触发 `ToolSetChanged`。
- Managed Playwright Browser Tool 通过内部 HostBridge 运行；敏感操作还需 per-Tool 或 Browser risk approval，并绑定当前 managed surface/origin、activation ID、schema/manifest digest。
- Managed Playwright/MCP Server 结果已经由 Main/Core Server 做安全 Artifact 投影，不进入普通 Exact Archive。截图可额外发布 `image-artifact://` readPath；下载等其他产物保留为 Run 生命周期的 Browser Artifact 引用，由 Main 的 broker 预览或导出。

## Skill Script 权限特例

`skills_preflight_script` 虽声明为不启动脚本的 ReadOnly Tool，当前依赖检查仍需检查宿主 Python 与安装元数据；`skills_preflight_script` 和 `skills_run_script` 因而都要求 `read=all`、`write=all`、`command safety=full_access`。`skills_run_script` 还固定为 `approval_mode=Always`，每次执行都要显式批准。两者均不安装缺失依赖；这些临时高权限前置条件不能由 Skill trust 或 Capability 激活替代。

## 命令风险与授权

命令解析器按完整 segment（换行、pipeline、`&&`、`||`、`;` 等）分类，而不是只看首个 token。风险类别包括只读、安全工作区写、直接写、网络、包管理、高影响、外部读取、未知、灾难性和不支持；策略结果为 allow、explicit approval 或 deny。

授权来源 `Automatic` 与 `ExplicitUser` 由 Core Server 绑定。批准后仍重新校验规范化命令、cwd、workspace root、timeout、输入 mount 和 frozen runtime profile。禁止后台 shell 语法绕过受管 Session；长任务通过 `run_command` 返回的 session ID 和 `command_session` 管理。

## 取消与不确定结果

Tool 声明两类 settlement：

- `Interruptible`：取消后可以停止等待，不存在未确认的外部/持久承诺；
- `Authoritative`：可能跨过副作用或持久提交边界，必须等待 Tool 自己的有界终态。

批准后执行、图像生成、Command Session、协作写入和某些 MCP Server/Managed Playwright 调用不能把“调用方已取消”等同于“动作未发生”。dispatch certainty 不足时返回 outcome/commit unknown，禁止 Runtime 自动 retry。`command_session interrupt` 是对已授权 session 的窄控制，不开启第二次审批。

## 不变量

1. Tool identity、permission 和 approval 均由 Core Server 决定，Tool 名和模型理由不构成授权。
2. prepare 的动作绑定 conversation/Run/call/action、资源 identity、revision 和 policy；执行前重新验证。
3. pending action 状态只通过事务 CAS 前进；批准必须先提交再执行副作用。
4. stable Tool prefix 与 dynamic revision 是请求契约，恢复时不能静默换定义。
5. AutoApprove 保留与手动审批相同的安全执行路径。
6. Event/Trace projection 无执行权；Checkpoint 只保存工具声明的安全投影。
7. outcome unknown 禁止自动重放。

## 代码真源

- Registry/trait/projection：`crates/core/src/tools/mod.rs`
- Effective toolset：`crates/core/src/tools/tool_set.rs`
- 权限和 proposed action：`crates/core/src/protocol.rs`
- 文件输入/写入/patch：`crates/core/src/file_input.rs`、`file_write.rs`、`patch.rs`
- 命令策略：`crates/core/src/command/policy.rs`、`risk.rs`、`spawn_plan.rs`
- Runtime 审批边界：`crates/core/src/runtime.rs`、`runtime/checkpoint/`
- Pending action：`crates/core/src/storage/service/pending_actions.rs`
- Core Server 执行：`crates/core-server/src/application/agent/action_execution/`
- MCP Server/Capability：`crates/core/src/tools/mcp.rs`、`builtin_capability.rs`、`crates/core-server/src/application/mcp/`

## 测试

- `crates/core/src/tools/mod.rs` 与 `tool_set.rs` 内契约测试
- `crates/core/src/tools/mcp/tests/`
- `crates/core/src/tools/run_command/tests/`
- `crates/core/src/runtime/tests/approval_resume.rs`
- `crates/core/src/runtime/checkpoint/tests/`
- `crates/core-server/src/application/agent/tests/file_write_permissions.rs`
- `crates/core-server/src/application/agent/tests/pending_actions.rs`
- `crates/core-server/src/application/agent/tests/mcp_approval_lifecycle.rs`
- `crates/core-server/src/application/agent/tests/cancellation.rs`

## 变更检查表

- [ ] 注册唯一 typed identity、portable schema、safety、approval mode 和 exposure。
- [ ] 声明 permission policy、取消 settlement 与 checkpoint persistence。
- [ ] 为 prepare/execute 绑定 call/action/Run/conversation 和资源 revision。
- [ ] 覆盖 denied、auto-approved、approved、rejected、expired、cancelled、crash-resume、unknown outcome。
- [ ] 实现 Model/Event/Trace/Archive/Checkpoint 投影并更新 projection fixture。
- [ ] Tool 列表/能力变更更新自动化 inventory，而不是仅更新文档表。
- [ ] 文件/命令/MCP Server 路径无 symlink、TOCTOU、secret 和跨会话授权绕过。
- [ ] 更新 Tool Result matrix/limits 和受影响子系统文档。

## 当前限制

- 权限主要是 Run/project 范围的本地桌面模型，不是多租户服务端 ACL。
- 外部 MCP Server 原始结果不进入 Exact Archive；重试审批调用可能产生新的外部副作用。
- 文件系统无法完全消除批准后到执行前的外部竞争，只能通过 identity/revision/no-follow 尽量 fail closed。
- 内置 Capability 当前使用 Run-bound、Core Server 进程内存 grant，不是永久授权；应用重启后必须重新取得 live grant，持久 pending/audit 只负责未完成审批的恢复。
- 完整 Tool inventory 尚以 Rust 注册代码和测试为真源，文档中的族列表不应被机器消费。
