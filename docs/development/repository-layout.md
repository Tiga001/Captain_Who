---
status: current
audience: developers
owner: engineering
last_verified: 2026-09-26
---

# 仓库结构

## 顶层目录

| 路径                              | 责任                                                                        |
| --------------------------------- | --------------------------------------------------------------------------- |
| `src/main/`                       | Electron Main、窗口、IPC、原生通知、浏览器、终端和 Core Server sidecar 宿主 |
| `src/preload/`                    | context-isolated、按领域拆分的 Renderer Host API bridge                     |
| `src/renderer/`                   | React 界面、Feature 控制器和展示状态                                        |
| `src/shared/`                     | Main 与 Renderer 共用的纯类型/本地化 Catalog，不依赖 Electron 进程能力      |
| `packages/host-api/`              | Renderer 可调用的 channel 与 Host API 类型                                  |
| `packages/protocol/`              | TypeScript 跨进程 DTO、严格 parser 和 fixture                               |
| `packages/artifact-runtime-node/` | 受管 Artifact Runtime 的 Node 入口                                          |
| `crates/protocol-rs/`             | Rust JSON-RPC 传输 DTO                                                      |
| `crates/core/`                    | Agent Runtime、工具、上下文、权限和 SQLite 领域实现                         |
| `crates/core-server/`             | Core Server 应用边界、JSON-RPC transport 与 Host adapters                   |
| `crates/mcp-client/`              | MCP 协议、连接、Catalog、Manager 与 stdio transport                         |
| `resources/`                      | 随应用分发的静态资源和受管组件描述                                          |
| `patches/`                        | pnpm 锁定依赖补丁；属于构建输入并由 runtime manifest/hash 绑定              |
| `scripts/`                        | 组件准备、测试门禁、打包、签名和验证脚本                                    |
| `build/`                          | Electron Builder 图标与平台 entitlement                                     |
| `docs/`                           | 当前开发文档、运维规则、ADR 和历史记录                                      |
| `public-docs/`                    | 受独立检查器约束的用户、集成、发布、支持、安全与第三方公开文档              |

构建缓存、`target/`、`out/`、`.cache/` 与测试临时产物不是代码真源，不应从中反推协议或版本。

## 依赖方向

```text
Renderer
  -> packages/host-api + packages/protocol
  -> Preload bridge
  -> Electron Main
  -> line-delimited JSON-RPC
  -> crates/protocol-rs
  -> core-server transport
  -> core-server application
  -> core + adapters

core-server -> mcp-client
core does not depend on Electron or Renderer
```

Renderer 内部依赖继续向共享层收敛：

```text
app -> features -> components -> config / host / protocol
```

`components` 不得导入 `features` 或 `app`，Feature 不得反向导入 App composition。ESLint 负责守住主要
边界；跨 Feature 的组合点由 AppShell 或明确的平台层拥有。

## Rust Core

[`crates/core/src`](../../crates/core/src) 主要领域包括：

- `runtime`、`llm`、`context`：模型运行、Provider 与上下文；
- `tools`、`file_change`、`command`、`skills`：Agent 能力、FileChange 事务与授权，包括 Automation 专用 `automation_report`；
- `storage`、`conversation_trace`、`world_state`：持久化与恢复真源，包括 Automation、workflow、模型可用性投影与受管附件导入；
- `workspace`、`workspace_instructions`：冻结的多目录工作区与根目录指令发现；`workflow`、`workflow_management`：工作流图定义、校验与实例配置；
- `office`、`artifact_runtime`、`image_generation`、`git_review`、`browser_downloads`：专项能力；
- `protocol`：依赖 Rust Core 概念的运行时模型，不属于跨语言 transport DTO。

[`crates/core-server/src`](../../crates/core-server/src) 不是通用 handler 集合：`transport` 负责 framing 和
路由，`application` 负责编排与生命周期（包括 AutomationService/Scheduler），`adapters` 负责 Git、
MCP、Skills、图片生成等外部实现。Main 的 `auth`、`attachments` 与 `workspaceFiles` 分别拥有账号/许可、原生文件导入与目录选择、工作区搜索等宿主边界。详见
[Core Server](../architecture/core-server.md)。

## Renderer Feature

[`src/renderer/src/features`](../../src/renderer/src/features) 按领域包含 chat、agentRun、
agentCollaboration、automations、browser、notifications、terminal、files、gitReview、mcp、skills、settings、
imageGeneration、auth、workflows 等。
共享 UI 放入 `components`；平台级组合、导航和 root-scoped store 放入 `app`。

Feature 不能通过读取另一个 Feature 的内部 store 建立隐式耦合。需要共享的数据应由权威 Host DTO、
App composition 或明确的公共契约提供。

## 新模块检查表

- 选择唯一所有者和依赖方向，避免在 Main、Core Server、Rust Core、Renderer 各实现一份业务真相；
- 跨进程能力先定义 DTO、parser、错误和版本，再接 UI；
- 对外副作用明确权限、审批、取消、超时和 `outcome_unknown`；
- 持久状态明确 schema、事务、重启恢复、删除与备份语义；
- 添加单元/集成/契约测试，并更新 [`docs/README.md`](../README.md) 中的权威文档入口。
