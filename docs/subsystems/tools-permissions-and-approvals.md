---
status: current
audience: developers
owner: engineering
last_verified: 2026-09-06
---

# Tool 体系、权限与审批

本文说明 Agent Tool 如何注册、暴露、授权、审批、执行、投影和恢复。具体 Tool 输出字段见[Tool Result 消费者矩阵](./tool-result-consumer-matrix.md)，大小限制见[Tool Result 上限、分页与恢复](./tool-result-limits.md)，文件写入的权威执行契约见[FileChange 子系统](./file-change.md)，后台任务的完整状态机见[Scheduled Automation 子系统](./scheduled-automations.md)。

## 职责边界

- 模型可以选择已暴露的 Tool 并提供参数，但不能授予文件、命令、网络、MCP Server、Capability 或 Skill 权限。
- `ToolRegistry` 拥有 Tool 身份、schema、暴露方式、权限域、审批准备、执行入口、取消结算和消费者投影。
- Runtime 冻结本次请求的 `EffectiveToolSet`，执行无需 Core Server 副作用的 Tool，并在审批点生成 Checkpoint。
- Core Server 持久化 pending action/audit，处理用户批准或拒绝，重新验证冻结动作，执行 Core Server-owned 副作用并提交终态。
- Renderer 只展示 Event 投影并提交用户决定，不持有执行凭据。

## 注册与暴露

每个 `AgentTool` 必须实现 `definition`、`permission_policy` 和 `exposure`；按需覆盖异步执行、审批准备、取消结算及六类消费者投影。

暴露方式：

| 类型                 | 含义                                                                                    |
| -------------------- | --------------------------------------------------------------------------------------- |
| `Stable`             | 权限允许时属于稳定 Tool 前缀；定义排序和 revision 是 Provider cache 契约                |
| `Dynamic`            | 由当前运行的扩展/目录动态加入，单独排序并参与 dynamic revision                          |
| `RequiresCapability` | 只有可信扩展提供当前能力快照，或经验证的 Skill/内置 Capability 激活后才进入动态 Toolset |

`EffectiveToolSet` 同时冻结 stable/dynamic definition、Tool typed identity、active capability 与整体 revision。模型只能调用该请求实际暴露的名称；“Registry 中有实现”不等于“本次请求已授权”。Capability ID 由后端从已验证 manifest 推导，不能接受模型自报 ID。

当前注册面可按以下族理解：

- 文件与检索：attachments、`read_*`、workspace/search、web；
- 写入与执行：唯一专用结构化文本文件修改 Tool `apply_patch`，以及可能产生独立副作用的 `run_command`、`command_session`；
- Office 与图像：三个 Office Tool、`image_generation`；
- Skills：resource list/read/materialize、script preflight/run、install prepare/commit，以及运行扩展 `skills_activate`；
- 历史与协作：`conversation_history`、spawn/send/followup/wait/list/interrupt；
- Scheduled Automation：仅在已原子 admission 的 Automation HumanRoot Run 中追加 `automation_report`；
- 内置能力：`activate_capability` 和激活后的 Managed Playwright Browser Tool；
- 外部扩展：MCP Server Tool 与其他 Runtime Extension。

基础 Registry 总是注册文件/搜索/写入/命令、Command Session 及 Skill Resource/Script Tool；Office、图像生成仅在对应 engine/execution 可用时注册。联网搜索扩展始终保留私有的 `web_search`/`web_fetch` 实现，是否暴露由当前 Host 能力快照决定。历史、Skill 安装、协作、Runtime Extension、Managed Playwright 和外部 MCP Server Tool 由 Core Server 在构造 Run 时追加。上述族列表不是可执行 allowlist；完整真源是 `ToolRegistry` 的注册调用、`EffectiveToolSet` 契约和相关测试，新增 Tool 必须让自动化检查发现，而不是只修改本文。

Git 差异通过 `run_command` 执行普通 Git 命令，沿用命令权限与审批规则。

### 联网搜索与浏览器动态开关

两个开关都是 Host 能力策略，不是模型稳定提示词中的行为偏好。每个自然模型请求边界先调用扩展的 `prepare_model_request`，由同一快照生成动态 Toolset、`RequestOnly` 专项说明和 World State 投影。开关本身不创建消息、唤醒 Run 或额外调用模型；稳定提示词与稳定 Tool 前缀不随开关变化，动态部分变化仍可能影响 Provider 对该部分的缓存。

- **联网搜索**：`WebSearchExtension` 从 Host 的 `WebSearchPolicySource` 读取开关与凭据可用性；仅两者满足时暴露 `web_search`、`web_fetch` 和搜索专项说明。`web.search` World State 用 `available/reason` 表达当前事实，变化由既有 World State diff 留在上下文。关闭、配置缺失或 Host 读取失败时移除工具和说明，分别呈现 `disabled_by_user`、`configuration_required`、`host_unavailable`。
- **浏览器自动化**：`BuiltinCapabilityExtension` 用同一请求快照提供可启用能力目录、动态 `activate_capability` 和已授权浏览器工具。关闭时不提供该能力的目录、激活入口和专项说明，只保留 World State 的能力 ID/状态事实。开启不等于当前任务已授权；关闭会使旧 live grant 失效，再开启仍按现有审批策略建立新 grant。没有任何可启用内置能力时，通用激活工具也退出当前 Toolset。
- **执行与恢复**：已发出的请求保持冻结的工具调用契约，但新搜索执行再次从 Host 读取实时策略与凭据；浏览器仍在 grant/dispatch 边界复核策略。关闭先于执行 admission 时，迟到调用在联网/dispatch 前拒绝；已接纳操作沿用原取消和结算生命周期。Checkpoint 只保存扩展壳和已接纳调用契约，不能恢复旧开关授权、浏览器 live grant 或 Tavily 密钥。审批/人机交互恢复后，下次模型请求读取最新设置，搜索设置变化不会使无关审批失效。

历史消息和已执行工具结果保留；卸载清理的是新请求的能力定义与专项说明，不改写历史事实，也不把这个开关扩展为对所有其他网络通道的统一禁令。联网搜索的稳定提示词不再重复扩展说明。图片生成继续由 Skill 管理，不参与这两个开关。

Core Server 为真实 Run、恢复和上下文预览提供 Storage-backed 搜索策略；设置 CAS 与执行 admission 共享设置锁，只读取搜索凭据，不把 Tavily 密钥放入 `AgentChatInput` 或恢复信封。无 Host 的 Rust Core 独立调用保留显式冻结配置适配器，该适配器不具备运行中更新能力，不能替代真实 Host 接线。

2026-09-06 验证：Rust Core 的 `web_search`（14 项）、`web_fetch`（10 项）、`prompts::tests`（18 项）、`builtin_capability`（29 项）及 `builtin_capabilities`（12 项）全部通过；Core Server 的 `web_search`（4 项）、`human_input`（34 项）、`pending_actions`（71 项）、`persisted_resume_input`（7 项）及 `builtin_capability`（35 项）全部通过，各 filter 存在重叠，不作为独立用例总数相加。真实 Harness/Host 测试使用 loopback Provider、临时 SQLite 和内存测试凭据，覆盖运行中开关、迟到搜索/抓取、审批及同步提问暂停、凭据缺失、重启和重新授权；没有调用真实搜索服务或启动真实浏览器。相关三包 all-targets Clippy、文档和测试布局检查通过。本轮没有修改 Renderer/Preload 协议，不新增数据库版本或要求重置开发数据。

## 权限模型

`AgentPermissions` 是后端权威值：

| 维度              | 当前值                              | 说明                                                              |
| ----------------- | ----------------------------------- | ----------------------------------------------------------------- |
| read              | `workspace_only` / `all`            | 工作区、附件与受管引用；`all` 才允许受支持的外部绝对路径/系统别名 |
| write             | `denied` / `workspace_only` / `all` | 控制所有 file-write 域 Tool；可见性不代表写权限                   |
| command           | `require_approval` / `auto_approve` | 用户正常审批偏好                                                  |
| command safety    | `guarded` / `full_access`           | 独立的风险上限；always-denied 操作不因 full access 放行           |
| patch             | `require_approval` / `auto_approve` | 结构化文件/Office 写入是否弹窗，仍保留路径与 revision 校验        |
| builtin execution | `require_approval` / `auto_approve` | 应用可证明来源的内置 Skill/Capability 是否弹窗；不扩大其他权限    |

模板、项目、父 Agent 和动态 policy 之间使用逐维 `meet`，只能收紧不能扩权。Tool 用 `AgentToolPermissionPolicy::Default` 或 `FileChange(ReadWrite|WriteOnly)` 声明权限域；禁止在 Runtime 中维护第二份按 Tool 名判断的易漂移 allowlist。

`builtin execution=auto_approve` 只把人工点击替换成 Host 自动批准。它不改变 Tool
定义、顺序或 schema，也不扩大 read/write/command/network/path 权限。自动路径仍创建同一 typed
action，复核 source/manifest/revision/digest，建立一次性 grant 或 durable dispatch receipt，并在
取消、过期、崩溃恢复时 fail closed。外部 MCP、workspace/installed Skill 和用户插件不因这个值获得
应用内置信任。

### Scheduled Automation 权限快照

Automation 的 `permissionModeVersion` 当前为 2。Core Server 在创建/更新时把 Composer 的三种模式解析成完整 `AgentPermissions`，同时保存安全 DTO；Run 入队时再把这份权限冻结进 `config_snapshot_json`：

| 模式      | 当前解析                                                                                                                  |
| --------- | ------------------------------------------------------------------------------------------------------------------------- |
| `default` | workspace read/write；command、patch、builtin execution 需审批；`guarded`                                                 |
| `full`    | read/write all；command、patch、builtin execution auto approve；`full_access`，且用户设置必须仍启用 Full                  |
| `custom`  | 复制当前自定义 read/write/command/patch/builtin execution，但无条件把 command safety 收紧为 `guarded`，且 Custom 必须启用 |

未来 Run 使用冻结权限，当前用户偏好只作为撤销上限，绝不能重新解析出更宽权限。Full/Custom 会在 scheduler precheck 和 HumanRoot `BEGIN IMMEDIATE` admission 事务中再次检查；若权限在两个检查之间被关闭，Task 与未 admission Run 在该事务内变为 blocked/failed，Conversation、message 和 Trace 不会写入。重新开启偏好不会自动修复已 blocked Task，用户必须提交一次有效更新。

## 文件与 URI 授权

模型路径先进入 `file_input`/filesystem router：

- workspace 相对路径；
- 已注册 attachment 路径；
- `skill://<package>/<revision>/<resource>`；
- `artifact://sha256/...` 或 `image-artifact://sha256/...`；
- 只有 read/write=`all` 时允许的受支持系统别名和外部绝对路径。

解析过程执行规范化、父目录与 symlink/reparse 检查、conversation grant 校验、文件 identity/revision 校验，并为命令/Office 创建私有只读输入 mount。当前一次文件输入最多 16 项，单项 64 MiB、合计 128 MiB；视觉输入单项上限为 8 MiB。URI 是稳定引用，不是裸本地路径，不能被字符串替换绕过授权。

模型通过专用结构化文本修改 Tool 写文件时只使用 `apply_patch`：Direct 模式用于一次性 create/update/delete，Staged 模式使用同一 FileChange transaction/store 分块组装，当前单事务目标上限 4 MiB。create/begin-create 不接收公开 Observation；Host 私下冻结 Missing/parent 状态并以 no-clobber 提交，成功后才签发第一个公开 ID。update/delete 与 begin-update 绑定准确 `read_file` Observation；成功 apply 或 Staged update commit 后，Runtime 重新验证写后目标并把同一个 ID 续约到新状态，供同一 Run 的后续模型响应使用。同一 Provider Tool Call 批次重复使用该 ID 会在副作用前拒绝。签发或续约失败只要求重新读取，不会把已经提交的结果误报为失败。两种模式始终绑定目标 scope、无 symlink 父链、基础 revision、frozen target/diff digest 和原子发布检查；create 始终 no-clobber，并拒绝不支持的 Office/PDF 二进制修改。AutoApprove 只跳过用户点击，不跳过 proposal、Pending、Checkpoint、dispatch claim 或执行前复核。命令与 Office/Builder 的文件副作用不进入这套 Observation/audit 契约，分别按自身边界授权和记录。完整 schema、状态、限额、持久表和历史 Diff 见 [FileChange 子系统](./file-change.md)。

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

当前 typed proposed action 包括普通 ToolCall、外部 MCP Server Tool、内置 Capability 激活、内置 MCP 敏感调用、Browser risk、FileChange、Command、Skill materialization/script/installation 和 Office operation。Direct 与 Staged 文件修改共用同一个 FileChange proposal/result，不存在 Diff/FileWrite 双 action。

“无需弹窗”不一定等于“直接执行”。自动 MCP Server 调用或需要冻结资源的动作仍必须先 prepare，以便 Core Server 获得一次性的权威 payload、TOCTOU 校验和审计身份。

人工审批票据与短生命周期执行材料是两类状态。MCP Server、Browser risk、内置 MCP 敏感调用和 Skill 安装的票据会保持 pending，直到用户决定或所属 Run 的取消/终态流程显式收口；sealed payload、进程内 grant 或已准备包过期不再替用户作决定。用户稍后批准但执行材料已不可用时，Host 必须提交一个 `definitely_not_dispatched` 的 failed Tool Result，并恢复模型继续处理，不能把票据重新标成“过期后请重试”或实际 dispatch。

FileChange 还允许用户对 create/update 选择“本 Run 剩余 `apply_patch`”。该选择先持久化无 authority 的 pending intent，只有当前 FileChange 以匹配 receipt 成功结算后才激活 Run grant；grant 只覆盖同 Run、同冻结权限/Toolset/Provider revision、同 workspace 或精确 external parent 下的后续 create/update，永不覆盖 delete。每次 effect boundary 都重新加载 durable grant；终态、取消、恢复身份不匹配或目录 identity 变化时撤销或 fail closed。详见 [FileChange 子系统](./file-change.md#4-审批与-run-grant)。

### 后台 Automation Approval 与报告

Scheduled Automation 复用普通 HumanRoot pending action、audit、Checkpoint 和审批恢复，不建立第二套审批系统：

- 默认/自定义权限要求审批时，Run 从 `running` 投影为 `waiting_for_approval`，写入 Run attention，并由 HumanRoot producer 原子记录共享的 `approval_required` Notification event；batch 决定原生投递，Renderer 不必保持打开。
- 用户批准或拒绝仍通过原 pending-action CAS。进程重启后，Scheduler 从 in-progress Trace 与 durable pending action 恢复 observer；公开状态中的 `waiting_for_approval` 不是新的执行授权。
- 离开等待态会确认对应 attention，并 suppress 尚未展示的 stale approval notification；Main 在真正展示通知前还必须调用 Host-only validation。
- `automation_report` 仅通过 `automation_run.agent_run_id` 解析出的 run-scoped sink 注册。普通 HumanRoot、子 Agent和 admission 前的 Run 都得不到该 Tool；approval continuation/restart 则可从相同绑定恢复。
- `automation_report` 是 `Stable`、`ReadOnly`、`approval_mode=Never`，只能首次成功写入 `no_change|important_update|completed` 与最多 2048 UTF-8 字节的安全 summary。它只能更新当前 Run 的报告/预览，不能修改 Task、schedule、权限或通知策略。

若模型未成功调用该 Tool，终态报告为 `unknown`；`important_updates` 通知策略会把 `unknown` 当作应通知结果。要可靠抑制“无变化”的成功通知，模型必须显式写入 `no_change`。

## MCP Server 与内置 Capability

- 外部 MCP Server Tool 使用 typed MCP identity；模型可见名不能用于判断来源。其原始参数由受限授权信封处理，普通 Checkpoint 禁止直接持久未知/外部 MCP 参数。
- `activate_capability` 的授权语义是当前任务批准；实际 live grant 绑定当前 Run、manifest digest 与 policy revision，只存在于 Core Server 进程内存。`builtin execution=auto_approve` 时 Host 自动结算同一个 typed action 并写 durable receipt；否则由用户点击。新 Run 仍需重新建立 grant，Checkpoint 不序列化该 grant。激活后 Tool 进入 dynamic Toolset 并触发 `ToolSetChanged`。
- Managed Playwright Browser Tool 通过内部 HostBridge 运行；敏感操作还需 per-Tool 或 Browser risk approval，并绑定当前 managed surface/origin、activation ID、schema/manifest digest。
- Managed Playwright/MCP Server 结果已经由 Main/Core Server 做安全 Artifact 投影，不进入普通 Exact Archive。截图可额外发布 `image-artifact://` readPath；受管下载由 Main 捕获、Rust Core 持久化，并以无宿主路径的 `browser-download:<uuid>` 引用进入文件输入。授权可来自当前 Conversation、同一 Agent task tree、同 Project，或 read=all 下的手工下载能力；opaque 引用本身不构成授权。

## Skill Script 权限特例

`skills_preflight_script` 虽声明为不启动脚本的 ReadOnly Tool，当前依赖检查仍需检查宿主 Python 与安装元数据；`skills_preflight_script` 和 `skills_run_script` 因而都要求 `read=all`、`write=all`、`command safety=full_access`。`skills_run_script` 的静态 Tool 定义仍固定为 `approval_mode=Always`，以保持模型工具前缀稳定。仅当 active resource session 再次证明脚本来自 exact `bundled:application`、trust 为 `Application`、revision 与 bytes digest 均一致，且 `builtin execution=auto_approve` 时，Runtime 才将该次 prepared action 交给自动 Host 执行。workspace、installed 或伪造 source proof 的脚本仍必须显式批准。两类工具均不安装缺失依赖；这些高权限前置条件不能由 Skill trust、Capability 激活或新开关替代。

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
8. 人工审批票据不会因临时执行材料 TTL 到期而替用户结算；晚批准缺少材料时只能产生 definitely-not-dispatched 失败。
9. FileChange successor Observation 只属于模型与私有 checkpoint；公共持久投影不得把它变成可复制 authority。

## 代码真源

- Registry/trait/projection：`crates/core/src/tools/mod.rs`
- Effective toolset：`crates/core/src/tools/tool_set.rs`
- 权限和 proposed action：`crates/core/src/protocol.rs`
- 文件输入与修改：`crates/core/src/file_input.rs`、`crates/core/src/file_change/`、`crates/core/src/tools/apply_patch.rs`、`crates/core/src/tools/file_change_staged.rs`
- 命令策略：`crates/core/src/command/policy.rs`、`risk.rs`、`spawn_plan.rs`
- Runtime 审批边界：`crates/core/src/runtime.rs`、`runtime/checkpoint/`
- Pending action：`crates/core/src/storage/service/pending_actions.rs`
- Core Server 执行：`crates/core-server/src/application/agent/action_execution/`
- MCP Server/Capability：`crates/core/src/tools/mcp.rs`、`builtin_capability.rs`、`crates/core-server/src/application/mcp/`
- Automation 权限/报告：`crates/core-server/src/application/automation/permissions.rs`、`crates/core/src/tools/automation_report.rs`、`crates/core-server/src/application/agent/automation_turn.rs`
- Automation 原子 admission：`crates/core/src/storage/service/conversations.rs`、`crates/core/src/storage/automation_repository.rs`

## 测试

- `crates/core/src/tools/mod.rs` 与 `tool_set.rs` 内契约测试
- `crates/core/src/tools/mcp/tests/`
- `crates/core/src/tools/run_command/tests/`
- `crates/core/src/runtime/tests/approval_resume.rs`
- `crates/core/src/runtime/checkpoint/tests/`
- `crates/core-server/src/application/agent/tests/file_change_permissions.rs`
- `crates/core-server/src/application/agent/tests/file_change_source_boundary.rs`
- `crates/core-server/src/application/agent/tests/staged_file_change_execution.rs`
- `crates/core-server/src/application/agent/tests/pending_actions.rs`
- `crates/core-server/src/application/agent/tests/mcp_approval_lifecycle.rs`
- `crates/core-server/src/application/agent/tests/cancellation.rs`
- `crates/core-server/src/application/automation/permissions.rs`
- `crates/core-server/src/application/automation/scheduler/tests.rs`
- `crates/core-server/src/application/agent/tests/automation_turn.rs`
- `crates/core/src/tools/automation_report.rs`

## 变更检查表

- [ ] 注册唯一 typed identity、portable schema、safety、approval mode 和 exposure。
- [ ] 声明 permission policy、取消 settlement 与 checkpoint persistence。
- [ ] 为 prepare/execute 绑定 call/action/Run/conversation 和资源 revision。
- [ ] 覆盖 denied、auto-approved、approved、rejected、expired、cancelled、crash-resume、unknown outcome。
- [ ] 人工票据与 sealed/process payload 分开测试；晚批准缺少执行材料时生成安全 failed Tool Result 而不 dispatch。
- [ ] 实现 Model/Event/Trace/Archive/Checkpoint 投影并更新 projection fixture。
- [ ] Tool 列表/能力变更更新自动化 inventory，而不是仅更新文档表。
- [ ] Automation Tool/权限变更覆盖 frozen snapshot、设置撤销 TOCTOU、后台 approval restart 和 `automation_report` 单次写入。
- [ ] 文件/命令/MCP Server 路径无 symlink、TOCTOU、secret 和跨会话授权绕过。
- [ ] FileChange Run grant 仅从 applied granting receipt 激活，effect boundary 复核且不授权 delete。
- [ ] 更新 Tool Result matrix/limits 和受影响子系统文档。

## 当前限制

- 权限主要是 Run/project 范围的本地桌面模型，不是多租户服务端 ACL。
- 外部 MCP Server 原始结果不进入 Exact Archive；重试审批调用可能产生新的外部副作用。
- 文件系统无法完全消除批准后到执行前的外部竞争，只能通过 identity/revision/no-follow 尽量 fail closed。
- 内置 Capability 当前使用 Run-bound、Core Server 进程内存 grant，不是永久授权；应用重启后必须重新取得 live grant，持久 pending/audit 只负责未完成审批的恢复。
- Automation 没有独立审批超时；Run 可以持续停在 `waiting_for_approval`，直到用户决定、删除/资源失效或正常关停恢复流程使其收敛。
- Automation 当前不支持独立 Tool allowlist、附件/显式 Skill 选择或逐任务 reasoning override；它复用目标 Conversation/模型与普通 Agent Toolset。
- 完整 Tool inventory 尚以 Rust 注册代码和测试为真源，文档中的族列表不应被机器消费。
