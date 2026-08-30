---
status: current
audience: developers
owner: engineering
last_verified: 2026-08-30
---

# Office 自动化与受管 Artifact

本文描述 Office 的 Provider-neutral 语义执行层、可复现 Artifact Runtime，以及 Office、Command、图像生成和 Managed Playwright 的不同产物边界。图像生成细节见[图像生成](./image-generation.md)，浏览器产物见[浏览器自动化](./browser-automation.md)。

## 职责边界

Office 子系统负责把 typed semantic intent 编译、批准并执行为受验证的文档检查或渲染；Artifact Runtime 负责发现和校验打包工具链；Generic Managed Artifact 子系统负责内容寻址发布与 conversation grant。三者都不从模型字符串推导权限，也不把私有物理路径作为公开 API。Office workspace output、Generic Managed Artifact、image-generation Artifact 和 Browser Artifact 是不同授权模型，不能混为一个 store。

## 四个容易混淆的概念

| 概念                     | 含义                                                              | 是否是用户产物 |
| ------------------------ | ----------------------------------------------------------------- | -------------- |
| Office Engine            | 把 Office 请求交给受管 OfficeCLI/Office document renderer 执行    | 否             |
| Artifact Runtime         | 应用打包并校验的 Node/Python/ripgrep 及依赖运行环境               | 否             |
| Generic Managed Artifact | `managed_artifacts`/grants 管理的内容寻址 PNG/JPEG/WebP/PDF       | 是             |
| Browser Artifact         | Main `BrowserArtifactBroker` 管理的 Run 生命周期 Browser 产物引用 | 是，短期       |

Artifact Runtime 只提供可复现 executable/dependency 发现和 preflight，不负责权限、进程启动或文件授权。Generic Managed Artifact URI 只提供受控读取能力，不暴露 runtime 或存储绝对路径。Office render 默认发布到已批准的 workspace/external final path；Browser Artifact 则由 Main broker 预览/导出，只有 Managed Playwright screenshot 会额外尝试发布 Generic Image readPath。

## Office 工具面

模型使用三个 Capability-gated Tool：

- `office_document`：DOCX；
- `office_spreadsheet`：XLSX/XLSM/CSV；
- `office_presentation`：PPTX。

当前模型可见 operation 已收窄为：

- DOCX 与 XLSX/XLSM/CSV：`inspect`、`render`；
- PPTX：`status`、`inspect`、`validate`、`render`。

每次调用都要求简短、单行的 `reason`，但 reason 只用于展示和审计，不构成授权。`status` 不生成 execution request；`inspect`/`validate` 是 ReadOnly；`render` 因发布 output 而准备当前严格 `OfficeOperation` frozen snapshot，并复用统一文件修改审批策略。`create` 及所有 mutation 已从模型 schema 移除，并以 `office.model_operation_removed` fail closed；崩溃恢复只接受当次持久化的精确冻结 Office snapshot，不解析旧 Wire 或猜测 action。新建和编辑由 bundled Skill 的 managed Python/MJS Builder/Editor 完成，而不是重新开放 Office Tool mutation。

内部 Office Engine 仍用 `OfficeOperation` 的 `help/create/view/get/query/validate/set/add/remove/move/swap` 安全集合表达 Rust Core-owned 编译结果和历史恢复；它不是模型可直接调用的 CLI allowlist。`install/config/watch/open/close/mcp/serve/server/raw/raw-set/add-part/batch/dump/merge` 等管理、驻留、网络或 raw OOXML 操作仍显式拒绝。

通用 `read_word`、`read_spreadsheet`、`read_presentation` 是文本读取 Tool，与 Office Engine 分开。PDF 不由旧 `read_pdf` Tool 处理；应通过 bundled PDF Skill、`run_command`、`read_image` 或 Office 的受管 PDF render 流程处理。

## 语义请求编译

模型不直接生成 OfficeCLI argv。当前 Tool 只解析扁平、typed 的检查/渲染 intent：document kind、operation、`filePath`、文档块/sheet/range/slide selector、render output/viewport、timeout 与 reason；然后编译为版本化 `OfficeExecutionRequest`。内部 mutation 类型只服务于当前冻结 snapshot 的精确崩溃恢复，不属于当前模型 schema，也不是旧 Wire 兼容入口。

```text
model semantic args
  -> validate current model-visible operation and document kind
      -> status: direct Office Engine status
      -> semantic request
          -> resolve input/output through file authority
          -> classify ReadOnly or OfficeOperationAccess::FileWrite
          -> compile Provider-neutral semantic request
          -> ReadOnly inspect / validate: direct Office Engine execution
          -> render output: freeze OfficePreparedExecution v6
              (provider/engine/workspace/file revisions, safe argv, paths, timeout)
              -> typed approval
              -> Core Server revalidation
              -> Office Engine / Office document renderer
          -> validate outputs and publish safe result
```

`OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION = 6`。Prepared request 中的路径使用 frozen binding/placeholder，实际私有输入 mount 由 Rust Core 在执行时解析。Provider ID、engine revision、workspace/input revision、document kind、operation、argv 和输出 disposition 必须一致；审批后任何变化都导致冲突，而不是按新文件执行旧请求。

## Office 执行与输出

OfficeCLI discovery 支持应用 packaged component、显式配置和受限开发路径；生产运行应优先受管组件并校验 executable identity/revision。执行时设置禁用更新与 resident 模式的环境，禁止 Office Provider 自行常驻或联网管理。

当前默认 timeout 120 秒、最大 600 秒；文档最大 512 MiB，argv 最多 256 项/128 KiB；render 页数、viewport、grid 和图片尺寸另有硬限。数值真源在 `office/execution.rs`。

执行结果包含 documentKind、operation、exit/stdout/stderr、输出角色/类型/readPath、页/尺寸/Office renderer revision、完整性和不确定性。stdout/stderr 走共享 Command capture。成功退出不自动证明产物有效：Rust Core 还需校验文件为非-symlink regular file、格式/大小/预期路径和 render coverage。

模型通过 DOCX `render` + `outputFormat=pdf` 请求 Core Server 管理的 DOCX-to-PDF 转换；内部仍编译为受审计的 `view=pdf` 语义，不把 `pdf` 当作任意 OfficeCLI operation。Presentation render/contact sheet 使用受管 Office document renderer 与固定调用上限；Presentation 编辑由 bundled Skill 的 managed editor 完成。

## Artifact Runtime

Runtime bundle 由 component receipt 描述，discovery 校验：bundle/build-input revision、每个文件 size/SHA-256、总文件数/总字节、依赖版本、license/辅助脚本以及最终 runtime fingerprint。

截至核验日 bundle 为 `2026.08.5`，包含 Node `22.23.1`、Python `3.12.13`、ripgrep `15.1.0` 和固定 Node/Python 依赖。受管 spreadsheet Python profile 正式提供 `openpyxl@3.1.5`、`xlsxwriter@3.2.9`、`numpy@2.5.2` 和 `pandas@3.0.5`；Excel authoring 与发布前验证仍由 `openpyxl` 承担。准确版本必须以 `artifact_runtime/discovery.rs` 和构建 receipt 为准，不能从本文复制到打包脚本。

Bundle/Runtime status 的 availability 只有 `available` 或 `unavailable`；版本不兼容、完整性错误和不支持平台通过结构化 `errorCode`/`recovery` 表达，不存在第三个 `incompatible` availability。调用方通过 `ArtifactRuntimeRequirement` 请求能力，再获得受验证 invocation；不得把 `$PATH` 上同名程序默认为受管 runtime，也不得让模型选择任意 executable。

## Managed Artifact 模型

当前 generic store 支持：

- Image：PNG、JPEG、WebP，URI `image-artifact://sha256/<digest>`；
- Document：PDF，URI `artifact://sha256/<digest>`。

Artifact ID 是 `sha256:<digest>`，物理对象位于私有 `objects/` 下。SQLite `managed_artifacts` 保存 kind、format、media type、size、hash、尺寸和 storage-relative path；`managed_artifact_grants` 将 Artifact 绑定 conversation/Run/call。

URI 不是全局公开地址。`file_input`、`read_image` 或其他消费者在每次解析时校验：URI scheme 与 kind 相符、conversation 有 grant、记录路径安全、文件非 symlink、size/hash/格式仍匹配。物理 absolute path 不离开 Rust Core StorageService/Core Server 边界。

## 发布状态机

```text
producer creates private regular file
  -> validate authority (conversation/Run/call)
  -> allocate private staging in objects root
  -> bounded copy, no-follow
  -> validate complete bytes and detected format
  -> SHA-256 + immutable metadata
  -> atomic rename-noreplace to <digest>.<ext>
      -> existing object: verify exact identity
  -> sync directory
  -> register artifact row
  -> create conversation grant
  -> return safe URI/readPath
```

失败的数据库注册可能留下不可达的 content-addressed object；重试是幂等的，后台/启动清理可以移除孤儿。反过来，数据库 grant 不能指向未经验证的 staging 文件。

Generic Image 当前最多 8 MiB，PDF 当前最多 128 MiB。独立的 image-generation store 默认下载上限 32 MiB，并有 DNS/redirect/尺寸/像素防护；它兼容相同 `image-artifact://` URI 模型，但有独立 execution journal 和 startup recovery。

## Artifact 生产者与消费者

| 生产者                          | 产物/引用                                         | 关键边界                                                            |
| ------------------------------- | ------------------------------------------------- | ------------------------------------------------------------------- |
| Office render                   | 已批准 final path 的图片/PDF `readPath`           | Office engine/renderer revision、coverage、OfficeOperation approval |
| `run_command` / managed builder | Generic PNG/JPEG/WebP/PDF Artifact                | before/after observation、completion hook、commit unknown           |
| Image generation                | 独立 generation image Artifact                    | paid request idempotency、下载网络策略、generation journal          |
| Managed Playwright              | Browser Artifact；screenshot 可另发 Generic Image | HostBridge 清洗、最多 16 refs、单 ref 128 MiB、Run 生命周期         |

Generic Image/PDF 的主要消费者是 `read_image`、`run_command` 和其他显式声明相应 file-input kind 的 Tool，以及受控导出流程；Office workspace `readPath` 继续按文件权限解析。Renderer 只消费安全 DTO。模型只能传回 Tool 返回的完整 URI/readPath，不应猜 digest、改 scheme 或使用内部 `savedPath`。Browser Artifact 不等同于 Generic Managed Artifact：Renderer 通过 Main broker 预览/导出，模型只有在 screenshot 返回 `image-artifact://` readPath 时才能交给 `read_image`。

## 图像生成衔接

`image_generation` 使用独立 profile/credential、Provider adapter 和 execution service。默认最大并发 2、总准入容量 8（包含执行中和等待准入的请求）、执行 timeout 5 分钟。Provider 成功 URL 先经受限 HTTP/DNS/redirect policy 下载到 staging，验证图片后发布并提交 execution/Artifact journal。

终态区分 succeeded、failed、cancelled、outcome_indeterminate 和 commit_indeterminate。Provider 可能已完成付费生成而本地未确认时不得自动重放。Credential 只以 opaque ref 进入 SQLite，secret 位于 Keychain/credential backend。

本文只维护 Artifact 交界；图像 Provider 的新增和配置应以 `image_generation/` 类型、adapter registry 和 Core Server 测试为准。

## 删除、分叉与保留

- 分叉复制可见历史及相关 Generic Managed Artifact grant，不依赖源 conversation 后续存在；具体复制事务由 fork service 决定。
- 删除 conversation 撤销/删除其 Generic grant；共享内容对象只有在无引用且满足保留策略时才能清理。
- 同一 digest 可被多个 Run/call/conversation 授权，但一个 grant 不授予其他 conversation。
- 临时 staging、命令 workspace、Office render scratch 和 Browser Artifact 有各自清理期；Browser ref 最长 24 小时且生命周期为 Run，不能当长期用户文件或自动随历史分叉。
- 当前没有面向用户的统一永久保留/导出 SLA；清理代码必须保守且以数据库引用为准。

## 不变量

1. Office 模型面只接受当前 advertised inspection/render schema，绝不直接透传 raw CLI/OOXML 或重新开放 retired mutation。
2. Office read/write access 由 operation 和 output 共同决定；render output 走严格 `OfficeOperation` frozen snapshot、统一文件修改审批策略和 Office transaction/engine 提交。内部 `OfficeOperationAccess::FileWrite` 只是当前 Office 风险分类，不是模型工具名；AutoApprove 不绕过文件校验。
3. Artifact Runtime 的完整性不等于执行授权。
4. Managed Artifact bytes 不可变，identity 为内容 hash，授权由 grant 单独表达。
5. 私有绝对路径、staging path 和 credential 不进入 Model/Event/Trace/Renderer。
6. 发布先验证并原子化内容，再注册/grant；失败可幂等重试和清理。
7. Artifact URI 每次消费都重新校验 kind、grant、size 和 hash。

## 代码真源

- Office domain/compiler：`crates/core/src/office/types.rs`、`semantic.rs`
- Office discovery/execution：`office/discovery.rs`、`execution.rs`、`render_runtime.rs`、`word_pdf_render_runtime.rs`
- Office Tool：`crates/core/src/tools/office.rs`
- Artifact Runtime：`crates/core/src/artifact_runtime/`
- Generic Artifact：`crates/core/src/storage/service/managed_artifacts.rs`、`storage/managed_artifact_repository.rs`
- File input/URI：`crates/core/src/file_input.rs`、`tools/read_image.rs`
- Command publication：`crates/core/src/command/artifact_observer.rs`、`managed_output_publication.rs`
- Browser projection/broker：`crates/core/src/browser_artifacts.rs`、`src/main/browser/BrowserArtifactBroker.ts`
- Image generation：`crates/core/src/image_generation/`、`tools/image_generation.rs`

## 测试

- `crates/core/src/office/tests/`
- `crates/core/src/command/managed_runtime/tests/office_transactions.rs`
- `crates/core/tests/artifact_runtime_smoke.rs`
- `crates/core/src/artifact_runtime/discovery.rs` 内 receipt/integrity 测试
- `crates/core/src/storage/service/managed_artifacts.rs` 内 publication/grant 测试
- `crates/core/src/file_input.rs` 与 `tools/read_image.rs` 的 URI 隔离测试
- `crates/core-server/src/application/agent/tests/office.rs`
- `crates/core-server/src/application/agent/tests/image_generation.rs`
- Core Server/Main 的 Managed Playwright screenshot/download 与 pending action 测试

## 变更检查表

- [ ] 新 Office intent 同步模型可见 schema、semantic compiler、prepared version、argv coverage 和 permission classification；retired operation 不得意外重新暴露。
- [ ] Provider/renderer 新操作不进入 raw/admin/network/resident 禁止面。
- [ ] 输出验证覆盖格式伪装、symlink、尺寸、页数、partial coverage 和 cancellation。
- [ ] Runtime bundle 变化更新 receipt、版本/hash、依赖/license 和 smoke test。
- [ ] 新 Artifact kind 定义 scheme、格式探测、大小限、读取工具和 grant 语义。
- [ ] 发布路径覆盖 staging、原子冲突、数据库失败、孤儿清理和幂等重试。
- [ ] fork/delete/retention 更新 grant/reference 测试。
- [ ] Model/Event/Trace/Renderer 去除 absolute path、secret 和 binary，并更新消费者 fixture。

## 当前限制

- Generic Managed Artifact 当前只支持 PNG/JPEG/WebP/PDF；DOCX/XLSX/PPTX 仍作为工作区文件输出，Office render 也首先是批准 final path，而非自动转成 Generic Artifact。
- Office Tool 当前只开放检查/验证/渲染；新建和编辑依赖已激活 bundled Skill 的 managed Builder/Editor。
- Office 依赖可用且通过校验的 OfficeCLI/Office document renderer；真实 Provider smoke test 在无受管组件环境可被标记为 ignored。
- Artifact Runtime 是本地打包环境，不是容器级沙箱；命令权限和进程控制仍是安全边界。
- 文件系统与 SQLite 不能共享单一事务，发布依赖 staging、原子 rename、幂等注册和 reconciliation。
- Artifact 统一跨设备同步、长期配额和用户级 garbage-collection 策略尚未形成公共契约。
