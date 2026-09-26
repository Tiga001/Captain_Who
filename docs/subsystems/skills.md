---
status: current
audience: developers
owner: engineering
last_verified: 2026-09-26
---

# Skills 平台

本文描述 Skill 从发现、冻结、激活到资源/脚本执行和安装更新的完整后端契约。Skill 是带来源和 revision 的指令包，不是任意目录提示词，也不是权限授予机制。通用 Tool/审批边界见[Tool 体系、权限与审批](./tools-permissions-and-approvals.md)，Artifact Runtime 与 bundled Office Builder 的边界见[Office 自动化与受管 Artifact](./office-and-artifacts.md)。

## 职责边界

- `SkillsService` 聚合来源、验证包、生成 catalog/discovery snapshot，并按精确 revision 解析。
- Runtime 将有限的发现目录加入上下文，通过 `skills_activate` 在 Run 范围激活包，并据此更新 Capability/Toolset。
- Resource runtime 负责 `skill://` 列表、读取和物化；Script runtime 使用经校验的宿主 Python 3 和结构化 argv。它不是 Artifact Runtime。
- Installation workflow 负责第三方来源解析、下载/读取、预览、批准和原子安装；模型不能直接写 managed store。
- Skill 的 trust 参与 policy，但绝不等于文件、命令、网络、Tool 或 Capability 授权。

## 来源、信任与作用域

| 来源      | 稳定来源标识                             | 作用域                 | 默认信任含义                              |
| --------- | ---------------------------------------- | ---------------------- | ----------------------------------------- |
| Workspace | 当前项目 `.agents/skills/<dir>/SKILL.md` | 当前 workspace/request | untrusted；项目可编辑内容                 |
| Bundled   | `bundled:application`                    | 应用安装               | application；仍受 Tool 权限约束           |
| Installed | `installed:user`                         | 用户 managed store     | user-approved；安装批准不自动批准脚本执行 |

同名 Skill 不应用模糊名称覆盖。Catalog 使用完整 `SkillId`、source kind、provenance 和 content revision；冲突、无效包和预算截断通过结构化 diagnostics 暴露。Workspace Skill 不发布到全局 installed store，离开当前 workspace 后不可发现。

多目录项目的 Workspace Skill 发现仍使用可用主目录；不能因为辅助目录或 Composer 文件夹引用可读，就认为其中的
`.agents/skills` 已加入 catalog。与之独立的 `workspace.instructions` 会从每个冻结项目根目录读取
`AGENTS.override.md`/`AGENTS.md`，无需 Skill 激活，但不增加权限或注册 Tool。详见
[工作区指令](workspace-instructions.md)。

Composer 的 `+` 首页和 `@` 菜单共用资源选择状态，Skill 显式选择继续引用权威发现项；菜单选择不会安装 Skill，
也不会跳过 Run 内的 revision 验证和激活。输入/队列边界见[会话输入](conversation-inputs.md)。

## 包格式与目录

入口必须使用精确大小写 `SKILL.md`。包可以包含：

```text
SKILL.md
references/**
assets/**
templates/**       # 作为安全 sibling resource 进入当前包格式
scripts/**
其他经验证的安全 sibling resource
```

当前支持 package format v1/v2/v3：

- v1：仅 `SKILL.md` 的兼容身份；
- v2：`SKILL.md` 加传统 `references/assets/scripts` 资源树；
- v3：允许其他经验证的安全 sibling resource，例如模板/能力描述文件。

Package revision 只由规范化逻辑路径、文件类型、长度、digest 和内容确定；权限位、mtime、扩展属性和空目录不参与。路径必须是可移植 UTF-8 相对路径，拒绝 `..`、反斜杠、控制字符、保留设备名、大小写碰撞、symlink/reparse 和目录逃逸。

当前主要安全限包括：`SKILL.md` 256 KiB、单 resource 16 MiB、整个包 64 MiB、最多 1,024 个文件、目录深度 16。数值真源是 `skills/workspace.rs` 与 `skills/package.rs`。

## 发现与冻结

```text
register bundled/installed sources
  + request-scoped workspace source
  -> bounded discovery + diagnostics
  -> sort by stable identity
  -> catalogRevision
  -> AgentSkillDiscoverySnapshot v2
  -> fit prompt token budget
  -> freeze activationRef -> (skill id, revision)
```

Discovery snapshot 当前默认最多占用 2,000 tokens，并根据 context window 进一步收紧。默认一次 Run 最多激活 8 个 Skill，激活源码合计 512 KiB。模型只看到短 activation ref、name、description 和 source kind；后端保留完整 ID/revision 映射。

发现时读到的本地路径不是长期 authority。解析/激活再次验证包 identity，并把内容复制为拥有所有权的不可变 snapshot；Checkpoint 恢复必须还原完全相同的 package/resource identity。

## 激活与同 Turn Toolset 更新

`skills_activate` 是 Runtime Extension，而不是普通文件读取。流程：

1. 模型提交 discovery snapshot 中的 opaque activation ref。
2. Runtime 校验 catalog revision、ID/revision、重复选择和激活预算。
3. 解析完整包，冻结 instructions、resource index 和 capability descriptor。
4. 发出 `SkillActivated`，更新 `ActivatedSkillSet` revision。
5. 对需要的 `skill.resources.*`、`skill.scripts`、Office、image 等 application-owned capability 求交。
6. 在同一 Run 的下一次请求生成新的 dynamic Toolset revision；不需要另起 Turn。

已激活包在该 Run 内保持 revision-bound。Workspace 文件随后变化不会静默替换运行中的指令；重新运行/重新发现才会得到新 revision。Checkpoint 恢复找不到精确 revision 时 fail closed。

若需要编辑 bundled/installed Skill，必须先将完整包复制到 workspace，再修改副本；不得直接改变应用 bundle 或全局 content-addressed store。

## Resource URI 与读取

`skill://` URI 绑定 package identity、revision 和逻辑资源路径。Tool 面：

- `skills_list_resources`：分页列出当前已激活包资源；
- `skills_read_resource`：UTF-8 文本按预算/字节游标读取；
- `skills_materialize_resource`：将选中资源写入获准目标。

URI 不暴露源绝对路径。每次读取重新校验当前 Run 激活、revision、digest、resource kind 和权限。二进制 asset 不应经文本读取伪装；需要落地时走 materialization 的 typed FileChange proposal，批准后再验证 source digest 与目标 revision。

## Skill Script

`skills_preflight_script` 不启动解释器、不导入模块也不安装依赖；它从宿主 `$PATH` 依次寻找工作区外的可执行 `python3`/`python`，校验解释器 identity，并以有界方式读取 distribution metadata/命令依赖。当前只支持 `scripts/*.py` 和逻辑 interpreter `python3`。该路径不是打包固定版本的 Artifact Runtime，也不是容器或 OS 进程沙箱。

`skills_preflight_script` 与 `skills_run_script` 当前都要求 `read=all`、`write=all` 和 `command safety=full_access`。前者的 Tool safety 仍是 ReadOnly 且不弹审批，但权限前置条件不会因此放宽。后者始终以 frozen `skill://` script、结构化 argv、声明 requirements、runtime fingerprint、timeout 和授权 identity 建立 proposed action；workspace/installed Skill 仍逐次要求明确批准，只有来源证明精确匹配 `bundled:application`、source kind/trust/SkillId 一致，且 `builtinExecution=auto_approve` 时，才可跳过本次点击自动执行。

`builtinExecution` 是与 `command`、`commandSafety`、`read`、`write` 和 `patch` 分离的权限维度。默认值是 `require_approval`；Full permission preset 将其设为 `auto_approve`，General Settings 的 Custom permission 可单独切换。AutoApprove 只省略 application-owned bundled Skill/插件工具或脚本的显式批准，不扩大路径、读写、命令、manifest、revision、digest、runtime fingerprint 或依赖安全边界。Automation permission snapshot 也包含该字段，并在 admission 时与当前权限 ceiling 求交。

执行前会重复 preflight，比较 source proof、Skill revision、脚本 digest、requirements 和 runtime fingerprint；缺失依赖只报告，不自动安装。脚本 cwd 固定为当前 workspace root，当前请求 schema 不接受任意 cwd 或 input mount。即使模型 Tool definition 仍声明 approval-required safety，Core Server 的 frozen dispatch 才是判断 exact application bundle 是否满足自动执行条件的权威，Renderer/模型不能自报 trust 来改变路由。

脚本 bytes 先复制到私有临时 snapshot，再以 Python isolated mode（`-I`）和结构化 argv 启动，不经 shell 拼接。当前默认 timeout 120 秒、最大 600 秒；最多 128 个参数、参数总计 64 KiB、requirements 最多 128 项。stdout/stderr 使用与命令相同的 128 KiB 预览和 64 MiB Exact Capture。取消终止进程组；若副作用可能发生，终态必须明确。

## 安装与更新状态机

第三方安装分成“检查”和“提交”两阶段：

```text
source URL / authorized local directory
  -> resolve source
      -> zero/one/multiple Skill candidates
  -> acquire exact immutable bytes
  -> validate package + provenance + warnings
  -> durable bounded preparation snapshot
  -> skills_prepare_install returns preview + opaque installRef
  -> model explains source/resources/scripts/warnings
  -> skills_commit_install(installRef)
  -> explicit user approval
  -> rehydrate exact prepared package
  -> managed installer CAS publish
  -> installed receipt + provenance/refresh authority
```

`skills_prepare_install` 永不安装。多候选来源返回 opaque `candidateRef`，用户选择后使用同一 source 再 prepare。`skills_commit_install` 只接受 `installRef`，模型不能重报 URL、digest、目标目录或 package bytes。

安装 store 使用内容寻址 package snapshot、不可变 generation 和原子 receipt publication。更新比较 expected installation revision；并发冲突返回 CAS 错误。Uninstall 保留 retired/tombstone identity，防止旧重试复活已卸载版本（ABA）。过期、取消或消费后的 preparation/session 不可再次提交。

GitHub acquisition 支持受控的仓库、目录或精确 `SKILL.md` URL，下载/解包有文件数、压缩比、路径、symlink、大小写和总字节限制。Provenance/refresh payload 经过 schema 和 digest 绑定；展示元数据不是下载 authority。

## Bundled Skill 维护

当前 bundle 包含 documents、pdf、presentations、spreadsheets、image-generation、skill-creator、skill-installer 等能力；准确清单以 `crates/core/src/skills/bundled.rs` 和 `crates/core/src/skills/bundled/` 为准。

新增/更新 bundled Skill 时：

1. 保持 `SKILL.md` frontmatter、名称和描述可用于低 token discovery；
2. 将模板、参考、脚本和 capability descriptor 纳入同一 package revision；
3. 区分两条执行路径：Skill Script 只声明其 Python distribution/command requirements；bundled Office/Artifact Builder 若使用 packaged Artifact Runtime，则通过受管 Command Profile 声明 requirement；
4. 真实编辑必须在 Skill 包副本中进行，不能要求模型改应用 bundle；
5. 更新 bundled catalog/resolve contract 与受影响 Office/Artifact 测试。

## 不变量

1. Skill revision 绑定整个包，不只绑定 `SKILL.md`。
2. 模型只能使用冻结 activation/install ref，不能自报信任、来源或 digest。
3. trust 不是权限；脚本、materialization、Office/图像能力仍走各自 policy。
4. Workspace Skill 不逃逸 workspace，不写入全局 managed store。
5. Installed package 内容不可变；current receipt 通过 CAS 切换 generation。
6. 准备、批准和 commit 绑定同一 package/provenance/revision；过期 ref fail closed。
7. 同一 Turn 激活改变 dynamic Toolset revision，恢复必须验证这一变化。
8. `builtinExecution=auto_approve` 只适用于 exact application-owned source proof；installed/workspace 脚本和任何校验漂移仍要求审批或 fail closed。

## 代码真源

- Domain/model：`crates/core/src/skills/model/`、`skills/package.rs`
- Discovery/source：`skills/service.rs`、`discovery.rs`、`workspace.rs`、`bundled.rs`、`installed.rs`
- Agent discovery/activation：`skills/agent_discovery.rs`、`resolver.rs`、`model/activation.rs`
- Resource/script：`skills/resource_runtime.rs`、`materialization/`、`script_runtime.rs`
- Installation：`skills/installation_workflow/`、`managed_installer/`、`managed_store.rs`
- Acquisition：`skills/github_source_resolution.rs`、`github_acquisition.rs`、`acquisition_provenance.rs`
- Tool adapters：`crates/core/src/tools/skills_*.rs`
- Script dispatch：`crates/core/src/runtime/command_dispatch.rs`、`crates/core-server/src/application/agent/action_execution/runners.rs`
- Core Server adapter：`crates/core-server/src/adapters/skills_adapter/`

## 测试

- `crates/core/tests/skills_resolve_contract.rs`
- `crates/core/src/skills/service/tests.rs`
- `crates/core/src/skills/materialization/tests.rs`
- `crates/core/src/skills/installation_workflow/tests/`
- `crates/core/src/skills/managed_installer/tests/`
- `crates/core/src/runtime/tests/skill_activation.rs`
- `crates/core/src/runtime/tests/builtin_capability.rs`
- `crates/core-server/src/application/agent/tests/skills.rs`
- `crates/core-server/src/application/agent/tests/pending_actions.rs` 的 Skill 恢复用例

## 变更检查表

- [ ] 包格式变更同步 format version、revision 算法、manifest 校验和旧格式读取测试。
- [ ] 新 resource tree 被分类、限长、digest，并覆盖跨平台路径碰撞。
- [ ] discovery prompt 仍在预算内，activation ref 与 catalog revision 绑定。
- [ ] 同一 Turn 激活、Checkpoint 恢复、包变化/缺失均有测试。
- [ ] 新 capability 由 Rust Core/Core Server 验证的 application manifest 推导，并与权限求交。
- [ ] script/materialize 使用 frozen URI、结构化输入、审批和 cancellation settlement。
- [ ] builtin-execution 变更覆盖默认/Full/Custom、exact bundled source、workspace/installed source、Automation ceiling、恢复与 source/digest 漂移。
- [ ] 安装来源新增 adapter 时实现 provenance、refresh schema、安全 capture 和多候选流程。
- [ ] prepare/commit/expiry/cancel/crash/CAS/uninstall-ABA 覆盖完整。
- [ ] 更新 bundled contract 和用户可见说明。

## 当前限制

- 激活作用域当前仅为单次 Run，不是跨会话常驻激活。
- Workspace 目录在读取期间仍可能被协作进程修改；通过 no-follow/identity/revision 检查 fail closed，但不是对恶意共享文件系统的强隔离。
- 安装源主要支持 GitHub 与已授权本地目录；其他 registry 需新增受审计 acquisition adapter。
- Skill Script 当前依赖工作区外 `$PATH` 上经校验的宿主 Python 3；版本未由应用 bundle 固定，也没有 OS 级进程沙箱，因此必须满足 unrestricted read/write + Full Access。workspace/installed 脚本仍逐次批准；只有 exact application-bundled script 可按 builtin-execution preference 自动执行。
- 模型 discovery 目录有固定 token/数量预算，超大 catalog 会有诊断和裁剪。
