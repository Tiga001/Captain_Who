---
title: 选择合适的能力
description: 判断何时使用普通 Tool、Skill、MCP、Multi-Agent、浏览器或 Scheduled Automation。
status: current
audience: user
owner: product-docs
last_verified: 2026-08-23
---

# 选择合适的能力

先问“任务缺的是什么”，再选择能力。不要因为功能看起来高级就默认叠加使用。

| 你的需要                     | 优先选择             | 原因                              |
| ---------------------------- | -------------------- | --------------------------------- |
| 读取、搜索或修改当前项目     | 普通 Tool            | 已有文件、命令和 Git 能力即可完成 |
| 复用固定流程、模板或领域说明 | Skill                | 把做法和资源打包，按任务激活      |
| 调用本地专用系统提供的工具   | MCP Server           | 为 Agent 增加结构化外部 Tool      |
| 在真实网页中导航或操作       | 内置浏览器自动化     | 使用受管浏览器与专门风险审批      |
| 多个独立调查需要并行         | Multi-Agent          | 子 Agent 分工，根 Agent 汇总      |
| 同一任务需要按时间重复       | Scheduled Automation | 持久 Task 按计划触发普通根 Run    |
| 只需一次简短回答             | 普通对话             | 最少配置和最低协调成本            |

## 常见组合

### Skill + Tool

Skill 规定“怎样做”，普通 Tool 负责读取文件或生成产物。例如文档 Skill 指导结构，文件 Tool 完成写入。

### Skill + MCP

Skill 规定团队操作流程，MCP 提供专用系统接口。两者仍分别受权限与审批约束。

### Multi-Agent + 普通 Tool

根 Agent 按前端、后端、测试拆分调查，每个子 Agent 使用相同基础工具。适合独立证据收集，不适合多人同时修改同一文件。

### Scheduled Automation + Multi-Agent

定时 Run 先启动根 Agent，根 Agent 再按需创建子 Agent。Scheduled Automation 不能直接绑定子 Agent 对话，也不提供额外并发保证。

## 不要混淆的名称

- **Scheduled Automation**：定时启动 Agent Run。
- **浏览器自动化**：让 Agent 操作受管网页。

它们是两个不同子系统；一个回答“什么时候运行”，另一个回答“怎样操作网页”。

## 最小能力原则

完成任务所需的最小能力通常也是最易验证的方案：

- 能用只读文件 Tool，就不必运行任意命令；
- 能用普通 Prompt，就不必创建永久 Skill；
- 能在一个 Agent 内完成，就不必拆分子 Agent；
- 不要求定期执行，就不必创建 Scheduled Automation；
- 不理解第三方 MCP Server 的行为，就不要启用自动调用。

各能力当前支持范围见[能力限制](../reference/capability-limits.md)。
