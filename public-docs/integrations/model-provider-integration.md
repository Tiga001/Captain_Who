---
title: 连接模型 Provider
description: 配置兼容模型端点、模型资料和凭据，并验证 Tool 能力。
status: current
audience: user
owner: product
last_verified: 2026-11-04
---

# 连接模型 Provider

Captain Who 不附带可直接使用的模型账户，新安装的模型目录也为空。你需要提供自己有权使用的 API URL、Token 和模型标识，并显式新增、启用模型；相关费用、数据处理和可用性由所选服务商决定。

## 当前适配类型

| 类型                       | 用途                                                |
| -------------------------- | --------------------------------------------------- |
| Generic OpenAI Chat        | OpenAI-compatible 的消息与 Tool 调用接口            |
| Generic Anthropic Messages | Anthropic-compatible 的 Messages 与 Tool Use 接口   |
| DeepSeek V4 Chat           | 需要保留特定推理续接语义的 DeepSeek V4 Chat         |
| Moonshot                   | 按模型族应用专用思考或推理强度设置的 Moonshot 配置  |
| 智谱                       | 国内标准 API 的 GLM-5.3 文本与 Flash 系列多模态配置 |

“兼容”指协议形状满足相应适配器要求，不代表 Captain Who 对所有网关、代理或服务商版本作认证。

## 配置全局连接

1. 打开“设置 → 配置”。
2. 在模型设置中填写 `API URL` 与 `API Token`。
3. 进入“管理模型”新增或编辑模型资料。
4. 在“可用模型”中启用需要使用的模型。

全局 URL 和 Token 会被没有单独覆盖值的模型继承。模型级连接必须同时填写 URL 和 Token；应用不会把模型级 URL 与全局 Token 混用。

## 新增模型

填写模型标识、显示名称和上下文窗口。模型标识会原样发送给 Provider；显示名称只用于界面展示。

在“更多”中选择 DeepSeek、Moonshot 或智谱后，顶部“厂商模型 ID”会显示软件已适配型号的下拉列表；通用兼容配置仍可手动输入。切换厂商不会自动选择第一个型号，原型号不适用时会提示重新选择。已有配置中的未知型号会保留显示，不会被静默替换。Moonshot 列表包含 Kimi K3.1 Pro 及其高速版，两者沿用 K3 的推理强度设置。

专用厂商的 URL 同时显示为只读官方地址：DeepSeek 为 `https://api.deepseek.com/chat/completions`，Moonshot 为国内端点 `https://api.moonshot.cn/v1/chat/completions`。自定义网关或其他区域端点请使用通用兼容。修改在保存后生效；原本继承相同官方全局地址的配置可继续继承整对连接，否则需要为模型配置对应的 API Token。

智谱固定使用 `https://open.bigmodel.cn/api/paas/v4/chat/completions`，请配置智谱的**标准 API Key**，按标准 API 规则计费。该配置不接入 Coding Plan 套餐或海外 Z.AI。[官方 Key 分类说明](https://docs.bigmodel.cn/cn/coding-plan/notice/api-key-management)解释了标准 Key、套餐 Key 与历史兼容 Key 的区别。

高级选项包括：

- 模型级 API URL 与 Token；
- API 厂商/兼容配置；
- 是否支持图片输入；
- 输入、缓存输入和输出的每千 Token 估算单价。

价格仅用于本地估算，不代表服务商账单，也不会自动识别币种。上下文窗口由用户配置；专用厂商的图片能力由选中型号决定，通用兼容允许手动配置。填写错误可能导致请求失败或容量判断不准确。

选择 DeepSeek V4 Chat 时，还可以设置推理模式和推理强度。选择 Moonshot 时，界面会按当前模型资料显示对应的思考或推理强度设置；不要把一个模型族的设置复制到另一个模型族。Provider 原生的私有推理续接不会被当作普通聊天文本公开展示，也不能无损迁移到所有其他 Provider。

## 智谱 GLM-5.3 配置

支持以下四款型号。上下文窗口可按官方 1M 配置为 `1000000`；显示名称、上下文和价格仍由你填写，应用不会覆盖已有值或自动转换以前的通用兼容配置。

| 模型 ID                | 图片输入 | 输入 / 缓存命中 / 输出参考单价（元/1k tokens） |
| ---------------------- | -------- | ---------------------------------------------- |
| `glm-5.3`              | 不支持   | 0.008 / 0.002 / 0.028                          |
| `glm-5.3-flash`        | 支持     | 0.0008 / 0.00023 / 0.0028                      |
| `glm-5.3-flashx`       | 支持     | 0.002 / 0.00057 / 0.007                        |
| `glm-5.3-flashx-legal` | 支持     | 0.002 / 0.00057 / 0.007                        |

参考价核对于 2026-11-04，后续以[智谱官方定价](https://docs.bigmodel.cn/cn/guide/start/pricing)为准。
Flash 系列的图片与 Legal 共用参数见[官方模型页](https://docs.bigmodel.cn/cn/guide/models/vlm/glm-5.3-flash)。
此次接入覆盖文本、图片和应用已有工具；选择 Legal 不会自动安装法律数据库或 MCP。

点击“厂商设置”调整推理强度：厂商默认、低、高或最高，新配置默认为最高。这一代模型强制开启思考，无法关闭；简单任务可选择低强度。应用自动保留必要的思考续接，支持多轮工具调用和审批恢复。

Token 详情中的缓存命中取自服务端用量。智谱未明确返回的缓存写入量不会显示为已知数量，缓存未命中不等于缓存写入；推理 Token 已包含在输出总量中。

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
