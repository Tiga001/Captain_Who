---
status: current
audience: maintainers
owner: engineering
last_verified: 2026-08-31
---

# 文档维护规范

## 目标

开发文档应回答“系统现在如何工作、为什么这样设计、怎样安全地修改和验证”，而不是重复代码或保存
过期计划。Markdown 是唯一维护源；PDF 仅在需要对外发布时由 Markdown 生成，不反向编辑。

仓库现在维护两棵边界不同的文档树：

- [`docs/`](../README.md) 面向开发者和维护者，可链接代码真源、内部限制、发布门禁与恢复步骤；
- [`public-docs/`](../../public-docs/README.md) 面向用户与集成开发者，只描述可验证的产品行为，不链接或泄露内部开发文档。

同一事实需要同时服务两类读者时，应分别按读者任务组织内容，并以代码为共同真源；不要让公开文档反向成为内部协议或安全边界的 authority。

## 文档分类

| 目录            | 责任                                 |
| --------------- | ------------------------------------ |
| `architecture/` | 跨模块边界、状态所有权和关键数据流   |
| `subsystems/`   | 单一业务或平台子系统的开发契约       |
| `development/`  | 环境、仓库、测试、构建和贡献流程     |
| `security/`     | 信任边界、威胁模型和安全不变量       |
| `operations/`   | 发布门禁、恢复和可重复操作步骤       |
| `adr/`          | 已接受且仍影响代码的架构决策         |
| `archive/`      | 历史计划和已替代设计，不作为当前真源 |

一个主题只保留一篇权威文档。跨主题内容使用链接，不复制容易漂移的版本号、工具全集、DDL 或协议
字符串。超过约 500 行且包含多个独立生命周期时，应拆分。

`public-docs/` 另按用户任务分为 `user/`、`integrations/`、`releases/`、`support/`、`security/` 和
`legal/`。每一级 `README.md` 都是该层索引；公开页面不得用占位内容承诺尚未建立的隐私政策、服务条款、
漏洞报告通道、版本生命周期或支持渠道。

## 必需元数据

每篇 Markdown 在标题前使用：

```yaml
---
status: current
audience: developers
owner: engineering
last_verified: YYYY-MM-DD
---
```

`last_verified` 表示已经对照代码和测试核验，而不是最后一次文字修改。历史文档使用
`status: historical`，草案使用 `status: draft`。

公开文档还必须提供 `title` 和 `description`，`audience` 只能是 `public`、`user` 或
`integration-developer`，并统一使用 `status: current`。未验证的公开主题应暂不创建，而不是发布 draft 或
空白占位页。

## 推荐结构

当前态技术文档通常包含：

1. 范围与非目标；
2. 模块和依赖边界；
3. 关键数据流或状态机；
4. 安全、持久化和并发不变量；
5. 代码与测试真源；
6. 当前限制；
7. 修改该领域时的检查表。

用 Mermaid 或短文本图表达跨三个以上模块的关系。图中的名称必须能映射到真实进程、模块或 DTO，
不要把计划能力画成当前能力。

## 术语与大小写

跨文档名称以[术语表](glossary.md)为准。普通叙述统一使用 `Agent`、根 Agent/子 Agent、`Turn`、`Run`、
`Tool`、`Skill`、`Provider`、`Artifact`、`Renderer`、`Preload`、`Main`、`Core Server`、`Rust Core`、
`MCP Server`、`Managed Playwright`、`Automation` 和 `Automation Run`。代码标识、协议字段与 UI
产品名称保持原始拼写；`Subagents` 仅指设置 UI 名称，不替代架构中的“子 Agent”，`Scheduled` 仅指
Automation 的产品入口名。

软件的叙述性产品名称统一为 `Captain Who`；根 package/data slug 为 `captain-who`，应用 Bundle ID 为
`io.github.tiga001.captainwho`。`mycopilot-*` crate、`@mycopilot/*`、`window.mycopilot`、
`MYCOPILOT_*`、仍保留的 `com.mycopilot.next.*` Keychain service、浏览器 partition/内部 URL 标记和摘要域
属于实现兼容标识，不随产品称谓做字符串替换。精确发行文件名以当前构建配置为真源。

首次出现的少数术语可以补充中文解释，例如“Artifact（制品）”，之后不要交替使用多个名称。SQLite
schema version、DTO `schemaVersion`、领域 `revision` 是不同概念，必须明确限定。

## 真源与链接

- 路径使用相对 Markdown 链接；不要只在反引号中写一个不可点击的关键路径。
- 版本和上限优先链接代码常量或生成文件。必须在正文写值时，同时说明真源。
- TypeScript/Rust 双端协议必须链接共享 fixture 或两端契约测试。
- 工具、RPC、IPC、表和测试全集不得靠人工清单宣称“完整”，除非有自动漂移检查。
- 外部规范只链接官方来源；文档中明确本项目已实现的子集。
- 公开文档的本地链接必须留在 `public-docs/` 边界内；允许的仓库外部文件目前仅是两份第三方声明。

## 变更触发器

以下改动必须在同一 PR/提交中检查文档：

- 新增或删除进程、crate、package、Feature 或 IPC/RPC；
- 修改 DTO、schema、状态机、权限、审批、安全边界或数据保留策略；
- 新增工具、Skill 来源、MCP transport、Provider、运行时组件或打包资源；
- 修改环境要求、命令、测试门禁、签名、发布或恢复流程；
- 用户可见能力、限制或默认值发生变化。

纯重构若不改变契约，可不改正文，但应确认代码链接仍然有效。

## ADR 规则

ADR 记录“为什么选定某个长期方案”，不充当操作手册。状态至少为 `proposed`、`accepted`、
`superseded` 或 `rejected`，并写明背景、决定、后果和替代方案。决定被替代时保留原 ADR，并链接新
ADR；不要悄悄重写历史。

## 完成标准

提交文档前至少完成：

- `pnpm check:docs` 通过；
- 修改公开文档时 `pnpm check:public-docs` 通过；
- Markdown 经 Prettier 检查；
- 所有本地链接和代码路径存在；
- 命令与 `package.json`/Cargo 配置一致；
- schema、协议、工具和运行时版本均有明确真源；
- “当前限制”与测试实际覆盖相符；
- 新文档已加入 [`docs/README.md`](../README.md)；
- 术语、大小写和状态名称符合[术语表](glossary.md)；
- 当前态、计划和历史没有混写；
- 未复制 Token、用户路径、日志中的敏感值或私有配置。

`pnpm check` 已包含 `pnpm check:docs`、`pnpm check:public-docs` 与 Agent avatar 校验；
`.github/workflows/tests.yml` 也复用这些仓库命令执行自动检查。仓库侧是否将对应 job 配置为 required check
不由 workflow 文件本身决定；提交者在合并或发布前仍应按改动域完成本地与专项门禁，不能维护另一套不同规则。

两套检查器的真源分别是 [`check-docs.mjs`](../../scripts/check-docs.mjs) 与
[`check-public-docs.mjs`](../../scripts/check-public-docs.mjs)。新增公开页面时应从所属 `README.md` 索引；若该页
属于发布必需集合，再更新后者的必需文档清单。删除尚未具备事实基础的页面时同步维护 forbidden placeholder
清单。
