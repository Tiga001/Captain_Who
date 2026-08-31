---
title: 术语表
description: 用简明语言解释 MyCopilot 用户文档中的核心概念。
status: current
audience: user
owner: product-docs
last_verified: 2026-08-31
---

# 术语表

## Agent

能够让模型在循环中观察环境、调用 Tool 并根据结果继续工作的执行主体。它不是单指某个模型。

## Agent Loop

模型决定下一步、Tool 执行、模型观察结果并继续的循环，直到完成、失败、取消或等待审批。

## Approval（审批）

用户对一个冻结的具体动作作出批准或拒绝。审批理由和模型文字不授予额外权限。

## Artifact（产物）

由 Tool 生成并由应用管理的图片、PDF 或其他结果引用。不同 Artifact 有各自的保留和导出规则；持久浏览器下载使用独立的 Browser Download 身份。

## Attention

Scheduled Automation 中需要用户查看的持久标记，例如等待审批、失败、重要更新或配置失效。标记为已处理不改变任务状态。

## Scheduled Automation Run

某条 Scheduled Automation Task 在定时、手动或恢复触发下产生的一次执行实例。

## Scheduled Automation Task

持久的定时任务配置，包含 Prompt、目标、模型、权限快照、计划和通知策略。

## Browser Automation（浏览器自动化）

Agent 通过应用受管浏览器执行导航、点击、读取或下载等操作的能力。它与 Scheduled Automation 不同。

## Browser Download（浏览器下载）

手动或 Agent 浏览产生并保存到配置目录的文件。模型只获得不含绝对路径的安全引用；下载记录与文件是否仍存在是两件事。

## Checkpoint（检查点）

Run 在审批或恢复边界保存的安全执行状态，使系统可以在重新校验后继续，而不必让模型重造动作。

## Compaction（上下文压缩）

把早期完整历史的模型视图替换为更短摘要，以腾出上下文容量。它不等于删除界面中的聊天记录。

## Context Window（上下文窗口）

模型一次请求可处理的信息容量，包含系统规则、Tool 定义、历史、附件说明、Tool 结果和预留输出。

## Conversation（对话）

持久保存用户消息、Agent 回答和相关运行记录的聊天容器。根 Agent 和每个子 Agent 各自绑定一个对话。

## Core Server

MyCopilot 本机运行的核心服务，负责 Agent Loop、权限、Tool 编排、定时调度和 SQLite 数据。用户通常不直接操作它。

## Destination（Scheduled Automation 目标）

Scheduled Automation 结果进入的位置：每次新建根聊天，或在一个现有根聊天中追加回合。

## Exact Archive

为受支持的执行内容保存的精确本地归档，用于历史核对。并非所有外部 MCP、浏览器或超大原始结果都会进入 Exact Archive。

## FileChange

Agent 创建、更新或删除单个文件时使用的统一事务。它绑定路径、文件版本、Diff、审批与终态，并在执行前再次检查冲突。

## Host

应用中真正连接操作系统、本地 Core 和受管外部能力的可信边界。模型提出动作，Host 负责验证和执行。

## MCP

Model Context Protocol，一种让应用发现并调用外部工具的协议。当前用户可添加本地 stdio MCP Server。

## Model / Provider

Model 是具体模型标识；Provider/Profile 描述与模型服务通信的协议和连接配置。MyCopilot 可以为多个模型保存不同配置。

## Multi-Agent

根 Agent 通过父子树把独立任务交给子 Agent，并通过持久消息和后续任务协调结果的机制。

## Outcome Unknown（结果未知）

动作可能已经产生外部副作用，但系统没有收到可确认的终态。应先检查实际环境，不能当普通失败直接重试。

## System Notification（系统通知）

操作系统显示的普通任务或 Automation 状态提醒。通知不是审批、任务真相或可靠的唯一告警通道。

## Project（项目）

用户授权给 MyCopilot 的本地工作区及其应用内元数据。移除项目不会删除源目录，但会删除关联的本地应用数据。

## 根 Agent（Root Agent）

一棵 Multi-Agent 任务树的入口，也是用户直接对话和处理审批的 Agent。

## Run

一次有明确开始、状态和终态的 Agent 执行。一个对话可以先后包含多个 Run。

## Skill

带来源和版本的指令包，可包含参考、模板、资源和可选脚本。Skill 教 Agent 怎样工作，但不授予权限。

## stdio

本地进程通过标准输入和标准输出交换协议消息的方式。当前用户 MCP Server 使用这种连接。

## 子 Agent（Subagent）

由父 Agent 创建并获得聚焦任务的 Agent 节点。用户可以只读观察其对话，但在根对话中协调和审批。

## Token

模型处理文本和结构化内容的计量单位，不等同于字符。上下文容量和用量统计通常以 Token 表示。

## Tool

Host 注册给模型的结构化能力，例如读文件、搜索、命令、MCP 或协作。模型只能提出调用，不能自己授予执行权限。

## Turn

对话中由一个输入触发并收口为一个结果的逻辑回合。一次 Turn 内的 Agent Run 可能包含多次模型请求和 Tool 调用。

## Workspace Skill

位于项目 `.agents/skills/` 下、只在当前项目发现的 Skill。它与全局 Installed Skill 分开管理。
