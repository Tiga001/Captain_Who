---
title: 连接模型 Provider
description: 配置兼容模型端点、模型资料和凭据，并验证 Tool 能力。
status: current
audience: user
owner: product
last_verified: 2026-09-03
---

# 连接模型 Provider

Captain Who 不附带可直接使用的模型账户，新安装的模型目录也为空。你需要提供自己有权使用的 API URL、Token 和模型标识，并显式新增、启用模型；相关费用、数据处理和可用性由所选服务商决定。

## 当前适配类型

| 类型                       | 用途                                              |
| -------------------------- | ------------------------------------------------- |
| Generic OpenAI Chat        | OpenAI-compatible 的消息与 Tool 调用接口          |
| Generic Anthropic Messages | Anthropic-compatible 的 Messages 与 Tool Use 接口 |
| DeepSeek V4 Chat           | 需要保留特定推理续接语义的 DeepSeek V4 Chat       |

“兼容”指协议形状满足相应适配器要求，不代表 Captain Who 对所有网关、代理或服务商版本作认证。

## 配置全局连接

1. 打开“设置 → 配置”。
2. 在模型设置中填写 `API URL` 与 `API Token`。
3. 进入“管理模型”新增或编辑模型资料。
4. 在“可用模型”中启用需要使用的模型。

全局 URL 和 Token 会被没有单独覆盖值的模型继承。模型级连接必须同时填写 URL 和 Token；应用不会把模型级 URL 与全局 Token 混用。

## 新增模型

填写模型标识、显示名称和上下文窗口。模型标识会原样发送给 Provider；显示名称只用于界面展示。

高级选项包括：

- 模型级 API URL 与 Token；
- API 厂商/兼容配置；
- 是否支持图片输入；
- 输入、缓存输入和输出的每千 Token 估算单价。

价格仅用于本地估算，不代表服务商账单，也不会自动识别币种。上下文窗口和图片支持同样由用户配置，填写错误可能导致请求失败或容量判断不准确。

选择 DeepSeek V4 Chat 时，还可以设置推理模式和推理强度。Provider 原生的私有推理续接不会被当作普通聊天文本公开展示，也不能无损迁移到所有其他 Provider。

## 验证配置

保存后先进行低风险测试：

1. 新建对话并选择该模型。
2. 发送一个简短问题，确认普通流式输出。
3. 请求一次只读 Tool 调用，确认 Tool Call 和 Tool Result 能闭环。
4. 再测试图片输入或较长上下文等已声明能力。
5. 对照服务商后台检查请求与计费。

配置完整只代表“可以尝试连接”，不代表远端 Token、模型名称或网络已验证。切换 Provider 时，如果现有对话包含另一 Provider 专属的续接状态，应用可能需要适配或拒绝直接切换。

## 数据与凭据

模型请求可能包含当前对话、系统说明、为任务读取的文件内容以及 Tool 结果的必要投影。只连接你信任并已阅读其数据政策的 Provider。

当前 SQLite 只保存模型凭据的不透明引用、状态和非秘密配置；密钥由独立 Credential Store 保存。具备稳定签名身份的发行构建使用操作系统凭据存储，未签名 macOS 开发构建使用权限收紧的私有文件。应用不会读回已有 Token；替换时只处理本次新输入。不要分享旧版数据库、完整数据目录或带 Token 的截图。详情见[数据与权限](../security/data-and-permissions.md)。
