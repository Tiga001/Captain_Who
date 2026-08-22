---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-23
---

# 图片生成子系统

图片生成由配置、凭据、Agent Tool 执行、Provider 适配、私有 Artifact 存储和 Renderer 展示组成。当前唯一适配器是 `smartmlSeedream`；Agent 通过固定 Tool `image_generation` 执行文生图或图生图，聊天界面只消费经过 Rust Core 验证和持久化的内部 Artifact，不直接加载 Provider URL。跨进程约束见 [IPC 与协议](../architecture/ipc-and-protocol.md)，UI 状态约束见 [前端架构](../architecture/frontend.md)。

## 职责与边界

| 层                 | 职责                                                                                       | 不负责                                           |
| ------------------ | ------------------------------------------------------------------------------------------ | ------------------------------------------------ |
| Rust Core          | 配置事务、凭据引用、执行快照、Provider 请求、下载与校验、Artifact 存储、执行日志和读取授权 | UI 表单状态、向 Renderer 暴露密钥或 Provider URL |
| Core Server        | 暴露配置与 Artifact RPC；将 `image_generation` 注册为 Agent Tool                           | 保存 Renderer 对象 URL                           |
| Electron Main      | 注册 Host API IPC；校验并把 Artifact 数据转换为 Host API 可传输的字节                      | 直接请求图片 Provider                            |
| Preload / Host API | 暴露类型化配置、状态和 Artifact 读取方法                                                   | 暴露文件路径或原始凭据                           |
| Renderer           | 编辑配置、展示可用性、解析 Tool 活动、按需读取图片、管理 Blob URL 生命周期                 | 信任模型输出或任意远程图片链接                   |

Bundled Skill `bundled:application:image-generation` 负责向模型说明 Tool 使用方式；真正的参数验证、授权、计费副作用防重和 Artifact 安全仍由代码负责。

## 配置与凭据

### 协议模型

协议版本为 1，当前只允许 `smartmlSeedream`。能力至少声明 `textToImage`，并可声明 `imageToImage`；默认尺寸预设为 `2K`，另有水印开关。可编辑的 `getConfiguration` 安全投影会返回 `endpointUrl`、`modelId`、启用状态、能力、默认项、凭据状态、readiness 和不透明 `revision`，以便 Renderer 编辑；它不会返回凭据内容或凭据存储引用。更紧凑的 `getStatus` 投影才会省略 endpoint 与 model。

readiness 包括：

- `disabled`
- `missingEndpoint`
- `missingModel`
- `missingCredential`
- `credentialUnavailable`
- `readyUnverified`

`readyUnverified` 只表示本地配置完整，不能解释为远端连通、模型名称或密钥已验证。

### 更新事务

配置保存在 SQLite，密钥保存在原生凭据后端，两者通过阶段化写入和启动期 reconciliation 保持一致。更新和启停都使用 `revision` 做 compare-and-swap；冲突时客户端必须重新读取，不得覆盖更新。

凭据变更协议支持 `keep`、`replace` 和 `clear`。Renderer 的配置 hook 串行化写入：启用且表单有未保存内容时，先更新配置，接收新 revision，再发启用请求；禁用不会隐式提交未保存字段。明文密钥只保留在表单内存中，并在成功替换后清空。

当前约束：endpoint 只接受 HTTPS，最长 2,048 字节；model 最长 512 字节；凭据最长 8,192 字节。启用前必须通过完整本地校验；凭据后端不可用时仍允许禁用，避免用户被锁在启用状态。

### 凭据后端

- 签名且使用固定应用身份的 macOS 构建使用非交互式 Mac Keychain v2。
- 未签名或开发态 macOS 构建使用应用私有文件凭据存储，避免 Keychain 身份漂移造成重复弹窗。
- 其他平台使用系统凭据存储实现。

SQLite 只保存带后端标签的不透明引用。读取时按标签 fail closed，不探测旧后端，也不把明文写入日志、协议响应或错误详情。

## Agent 执行流程

1. Renderer 启动 Agent Run；Rust Core 根据当前 Tool 集和 bundled Skill 向模型暴露 `image_generation`。
2. Tool 解析 `generate` 或 `edit` 参数。prompt 最长 32 KiB；图生图输入最大 20 MiB，并必须通过现有文件授权边界解析 `inputPath`。
3. Rust Core 创建配置执行快照：读取配置与 revision、解析凭据，再复读配置；若中途变化只重试一次，最终执行绑定到一个精确 revision。
4. 调度器按 Run/call 身份准入。默认最多并发 2 个请求、总计接纳 8 个执行（包含正在运行和等待中的请求）；配置步骤默认 15 秒，完整执行默认 5 分钟。
5. 适配器发起 Provider 请求。原始响应和远程 URL 只在 Rust Core 内处理，密钥在不再需要时清零。
6. 下载器按网络策略检查每次解析与重定向，完整下载后解码、验证格式和尺寸。
7. 图片以内容哈希原子写入不可变私有存储，生成 `image-artifact://sha256/<hash>` 身份。
8. Renderer event 投影只返回安全 Artifact 身份；内部 durable Tool history 为模型恢复兼容还会保留受管 `savedPath`，但 event projection 会在进入 Renderer 前删除该字段。Renderer 再通过精确身份和会话权限读取字节并展示。

### 副作用与重试

Provider 调用可能产生费用，网络超时不代表请求未成功。执行日志以 Run/call 身份记录准入、开始和结果，并区分 `succeeded`、`failed`、`cancelled`、`outcomeIndeterminate` 和 `commitIndeterminate`。一旦远端生成可能已发生，系统不得自动重放请求来“修复”不确定结果，否则可能重复计费和生成。调用方必须把不确定状态原样呈现。

## Artifact 安全与读取

Provider 返回的 URL 永远是不可信输入。下载只允许公开 HTTPS 目标，并对初始地址、DNS 解析结果和最多 3 次重定向逐跳检查，阻止私网、回环、链路本地等 SSRF 目标。默认限制包括：

- 最大下载约 32 MiB。
- 连接约 10 秒、DNS 约 5 秒、下载约 60 秒。
- 宽高各不超过 16,384 像素，总像素不超过约 6,400 万。
- 解码分配预算约 256 MiB。
- 仅接受协议允许并经完整解码确认的 PNG、JPEG 或 WebP。

文件扩展名、Content-Type 和 Provider 声明都不能代替完整解码。Artifact 使用内容寻址和原子落盘；同一哈希不可被覆盖为不同字节。

当前 Renderer 读取总会提交 `conversationId`。命令生成的图片和文档 Artifact 必须命中该 Conversation 的精确授权；为兼容旧版持久记录，历史图片生成 Artifact 可以省略 `conversationId`，但 Rust Core 只会在遗留图片发布日志存在且摘要、大小与类型完全匹配时读取。观察子 Agent 时必须同时提交子 Agent 的精确 `conversationId` 和 `observerRootConversationId`；后者本身不授予访问权，Core Server 会先验证该 Conversation 是对应根 Agent 的直接或传递子节点。跨树访问对调用方表现为不可见或 `notFound`，不会泄露路径、Provider URL 或是否存在。

Core Server 将 Artifact 转换为传输数据，Main 在进入 Host API 前再次校验并解码为字节。Renderer 最多并发解析 2 个 Artifact，把字节复制到 Blob，创建对象 URL，并以引用计数管理；组件卸载、身份变化或会话切换必须 revoke。图片卡片接近视口约 800 px 时只预取一次，读取缓存的 authority key 必须包含 Conversation、`observerRootConversationId` 和完整 Artifact 身份。

## 状态与安全不变量

1. API key、凭据引用、Provider URL 和图生图输入字节不得进入 Renderer 事件投影、Renderer 状态、持久化 Conversation 或日志。受管 `savedPath` 只允许存在于 Rust Core 内部持久 Tool 历史以支持模型恢复；进入 Renderer 前必须删除。
2. 配置写入必须携带 revision；冲突后重新读取，不能 last-write-wins。
3. “可启用”只基于本地校验；`readyUnverified` 不能展示为远端验证成功。
4. 图生图输入必须经过与文件 Tool 相同的工作区授权，不能直接读取模型提供的任意路径。
5. 远端下载必须逐跳执行公开 HTTPS 与 DNS 风险检查；不能只检查原始 URL。
6. 只有完全下载、解码和验证成功的字节才能提交为 Artifact。
7. Artifact 读取必须验证精确身份和 Conversation 树权限；UI 缓存不能扩大权限；省略 `conversationId` 的兼容读取只适用于遗留图片生成发布日志。
8. 对可能已产生费用的未知结果不得自动重试。
9. 异步配置、Tool 活动和图片读取结果写入 UI 前必须验证当前 revision、Conversation 和 Artifact 身份。

## 代码真源

- 协议：`packages/protocol/src/image-generation/contracts.ts`
- 协议解析：`packages/protocol/src/image-generation/parsers.ts`
- Rust Core 配置事务：`crates/core/src/image_generation/configuration.rs`
- Rust Core 凭据后端：`crates/core/src/image_generation/credential_store.rs`
- Rust Core 执行与日志：`crates/core/src/image_generation/execution.rs`
- Rust Core 类型与限制：`crates/core/src/image_generation/types.rs`
- Seedream 适配器：`crates/core/src/image_generation/seedream.rs`
- Artifact 下载、校验与存储：`crates/core/src/image_generation/artifact.rs`
- Agent Tool：`crates/core/src/tools/image_generation.rs`
- Core Server RPC：`crates/core-server/src/transport/image_generation_rpc.rs`
- Main IPC / Core Server 桥接：`src/main/ipc/serviceIpc.ts`、`src/main/core/coreServer.ts`
- Renderer 配置：`src/renderer/src/features/imageGeneration/configuration/useImageGenerationConfiguration.ts`
- 设置页面：`src/renderer/src/features/settings/pages/configuration/ImageGenerationSettings.tsx`
- Renderer Artifact 与活动：`src/renderer/src/features/imageGeneration/`、`src/renderer/src/features/chat/components/ImageGenerationArtifactsCard.tsx`

字段和限制以协议与代码为准。若目录内部进一步拆分，应保持上述领域入口可追踪，并同步本文。

## 测试

修改时至少覆盖：

- `packages/protocol/src/image-generation/*.test.ts`：严格解析、敏感字段拒绝、状态枚举和身份校验。
- `crates/core/src/image_generation/` 内测试：CAS、reconciliation、凭据后端、执行快照、SSRF、重定向、解码上界、内容寻址和权限。
- `crates/core/src/tools/image_generation.rs` 附近测试：参数限制、文件授权、安全投影和不确定结果。
- `crates/core-server/src/transport/image_generation_rpc.rs` 附近测试：配置/Artifact RPC、遗留图片兼容读取与 Conversation 树授权。
- `src/renderer/src/app/__tests__/ImageGenerationSettings.browser.test.tsx`：表单脏状态、串行 mutation、revision 冲突和明文清理。
- `src/renderer/src/app/__tests__/ImageGenerationActivity.browser.test.tsx`、`ImagePreviewLease.browser.test.tsx`、`hostImageArtifactResolver.test.ts`：活动解析、authority 隔离、预取、对象 URL 回收和失败状态。
- `src/preload/ImageGenerationIpcBridge.test.ts`、`src/main/core/ipc.imageGeneration.test.ts`、`coreServer.imageGeneration.test.ts`：Host API 边界与 Core Server 转发。

手工验证至少包含：未配置、缺密钥、凭据后端不可用、revision 冲突、文生图、图生图、取消、未知结果、跨 Conversation 读取拒绝、遗留图片兼容读取和应用重启后 Artifact 读取。

## 变更检查表

- [ ] 协议变化保持严格解析；任何响应都不含密钥、凭据引用或托管路径。可编辑的 `getConfiguration` 可含 endpoint/model，`getStatus` 和 Renderer Tool 事件仍不得暴露 Provider URL。
- [ ] 配置字段变化同时更新 SQLite schema/reconciliation、凭据引用、CAS 和设置 UI。
- [ ] 新 Provider 适配器仍通过统一执行快照、调度、日志、下载器和 Artifact 存储，不绕过公共安全层。
- [ ] 新网络路径覆盖 DNS 重绑定、重定向、私网目标、超时、大小和解码预算。
- [ ] 新图生图输入形式复用文件授权，并有大小与媒体校验。
- [ ] 任何重试策略证明不会重复产生收费副作用。
- [ ] Artifact 身份或授权变化同步更新 Rust Core、Core Server RPC、Host API、Renderer authority key 和测试。
- [ ] Renderer 的 Blob URL 在卸载、身份变化和错误路径上均被回收。
- [ ] 更新协议、Rust Core、Core Server、Main、Renderer 测试及本文限制。

## 当前限制

- 只有一个配置档和 `smartmlSeedream` 适配器。
- 默认输出尺寸固定为 `2K` 预设；没有通用的任意宽高协议。
- 设置 UI 支持保留或替换密钥；协议虽支持 `clear`，当前界面未提供独立清除入口。
- `readyUnverified` 不执行 Provider 连通或凭据有效性探测，首次真实执行才会暴露远端错误。
- 默认最多同时执行 2 个图片请求，总计最多准入 8 个；容量满时新请求以 `busy` 拒绝。
- Artifact 只支持 PNG、JPEG 和 WebP，并受下载、尺寸、像素和内存预算限制。
- 不确定结果不会自动重试，用户可能需要在 Provider 侧核对是否已计费或已生成。
