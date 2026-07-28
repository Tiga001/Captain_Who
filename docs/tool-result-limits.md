# Tool Result 上限、投影与恢复契约

状态：Round 3 生产契约

日期：2026-07-28

本文定义所有静态 Tool、Runtime Extension Tool、审批后 Host Tool，以及未来
MCP/Provider Tool 的统一结果限制。字段消费者归属见
[Tool Result 消费者矩阵](tool-result-consumer-matrix.md)。

## 1. 唯一主链路

```text
Tool / Provider
  → 必要的来源安全捕获或语义分页
  → Canonical Tool Result
  ├─ Exact Archive：保存安全捕获到的原文
  ├─ Event / Trace / Checkpoint：各自消费者投影
  └─ Model Projection
       → 固定 10K Model Result Gate
       → 当前模型历史与 Usage/Context 计量
```

系统没有每模型或每工具的模型 token 配置，也没有多级模型结果阈值。任何静态、
动态、审批后 Host Tool 的最终模型 observation 都必须经过同一个 10K Gate。

## 2. 四种上限语义

- **Source Truncation**：Tool 或 Provider 在 Archive 前不可恢复地丢失内容。
  必须提供 `truncatedAtSource=true`、已知遗漏量和停止原因。
- **Semantic Pagination**：完整结果集被稳定分页，返回 opaque `nextCursor`。
  Cursor 可恢复时不能标记为不可恢复的来源截断。
- **Consumer Projection Limit**：Renderer、Trace、Checkpoint 等消费者只需要有界
  视图。它不能改变 Canonical Result 或 Exact Archive。
- **Model Projection Truncation**：最终模型结果超过固定 10K。中央 Gate 负责安全
  裁剪，并保留 `nextCursor`、`continueWith` 或 `historyOpen`。

`archivedCompletely=true` 只表示后端完整归档了“实际收到且通过安全捕获的内容”，
不证明上游网页、Provider 或外部进程产生过但未返回的内容完整。
Provider 没有声明完整性时，Canonical Result 必须保留
`sourceCompleteness: "unknown"`，不得为了适配现有布尔 Archive Descriptor 而注入
`truncatedAtSource: false`。Descriptor 中的 `truncatedAtSource=false` 只表示后端没有
观察到已声明的来源丢失，不等于证明上游来源完整。

## 3. 主要工具策略

| Tool/类别                               | Source Capture Policy                         | Model Delivery Policy                                                | Overflow Recovery                                                | 主要消费者  | 安全硬限                                                                                         |
| --------------------------------------- | --------------------------------------------- | -------------------------------------------------------------------- | ---------------------------------------------------------------- | ----------- | ------------------------------------------------------------------------------------------------ |
| `search_code`                           | 枚举授权范围并读取 UTF-8 文本；跳过项显式计数 | 预算感知结果页，再经 10K Gate                                        | opaque `nextCursor`；超长行可按 path/line 用 `read_file` 恢复    | M/E/T/A/C   | walk 20,000 项；单文件 512 KiB                                                                   |
| `search_files`                          | 枚举授权目录的稳定快照                        | 预算感知结果页，再经 10K Gate                                        | opaque `nextCursor`                                              | M/E/T/A/C   | walk 20,000 项                                                                                   |
| `attachments_list*`                     | 读取当前授权附件目录快照                      | 预算感知结果页，再经 10K Gate                                        | opaque `nextCursor`                                              | M/E/T/A/C   | 单页请求上限 500                                                                                 |
| `workspace_map`                         | 有界 walk 后生成统计摘要                      | summary + treeText + coverage                                        | 缩小 `focusPath` 重新调用；不提供组内 cursor                     | M/E/T/A/C   | depth 8、tree 1,000、walk 20,000                                                                 |
| Command Artifact Observation            | before/after 有界扫描、哈希和格式校验         | 精简 changes/expected outputs + 完整性汇总                           | 无 cursor；`partial` 和 `stopReasons` 明确证据边界               | M/E/T/A/C   | 每快照目录项 100,000、Office 文件 2,048、哈希 512 MiB、单文件 128 MiB、2 秒；报告 256 项/128 KiB |
| `web_search`                            | 保存 Provider 实际返回的搜索产品结果          | 请求 1–8 条；Model 防御性最多交付 8 条并返回 coverage，再经 10K Gate | Provider 自带 cursor 原样保留；网页正文用 `web_fetch`            | M/E/T/A/C   | Model/Provider 请求结果数最多 8；Archive 保留实际收到的 Provider 页；Provider 自有上限           |
| `web_fetch`                             | 正文在模型投影前进入共享 Exact Capture        | 完整 canonical 正文交 10K Gate；Event/Checkpoint 独立有界            | `historyOpen`                                                    | M/E/T/A/C   | Exact Capture 64 MiB；传输/Provider 自有上限                                                     |
| 文档读取                                | 完整解析文本进入共享 Exact Capture            | 文本交 10K Gate                                                      | `historyOpen`                                                    | M/E/T/A/C   | 文件 25 MiB；OOXML 单 XML 16 MiB、总 XML 64 MiB；Exact Capture 64 MiB                            |
| `git_diff`                              | stdout 持续捕获到 spool 后归档                | 预算内 diff                                                          | `historyOpen`；按 path 重新查询                                  | M/E/T/A/C   | stdout/stderr 共享 Exact Capture 64 MiB                                                          |
| `run_command`、Office CLI、Skill Script | stdout/stderr 流式写入共享 spool              | 128 KiB/stream 预览或预算内输出                                      | `historyOpen`                                                    | M/E/T/A/C   | stdout/stderr 共享 Exact Capture 64 MiB                                                          |
| `read_file`、Skill Resource             | UTF-8 安全的预算感知语义分页                  | 当前页不再被中央 Gate 二次截掉                                       | `continueWith`/next byte cursor                                  | M/E/T/A/C   | 文件/资源读取自身安全限                                                                          |
| `conversation_history`                  | 读取 Message、Trace 或 Exact Archive          | 预算感知历史页                                                       | opaque `open`/navigation                                         | M/E/T/C     | 历史页与 Archive 分块限                                                                          |
| 动态 Extension / MCP-style Tool         | 原样接收并归档 Provider Tool Result           | Tool 投影后统一经过 10K Gate                                         | Provider `cursor`/`next`/`continueWith` 或 Archive `historyOpen` | M/E/R/T/A/C | Provider 声明的安全限 + 中央 10K                                                                 |

表中 M/E/R/T/A/C 分别表示 Model、Renderer/Event、Runtime Extension、Durable
Trace、Exact Archive、Checkpoint。

## 4. Opaque cursor 契约

本地搜索和附件列表只暴露一个可选 `cursor` 字符串：

- cursor 绑定工具类型、规范化 query/过滤条件、授权 scope、必要的 workspace 或目录
  revision，以及下一页位置；
- continuation 仍重新执行正常路径和附件权限校验；
- 结果快照变化、查询变化、跨 conversation/project 使用或 cursor 被修改时，返回结构化
  失效错误，要求移除 cursor 并重新搜索；
- `total` 是稳定结果集总数，`returned` 是当前页数量，`omitted` 是当前响应未包含的数量；
- `nextCursor` 只从模型实际收到的最后一项之后继续，不能被中央 Gate 二次裁剪后跳项；
- cursor 是定位信息，不是授权能力，也不允许模型解析或构造。

## 5. Exact Archive 与递归防护

正文型结果先使用共享 64 MiB Exact Capture，再写入现有
`conversation_history_blobs`/chunks，使用 zstd 无损压缩和 SHA-256 校验。进程输出在
执行期间写入临时 spool，不以 128 KiB 展示预览决定 Archive 完整度。

`conversation_history` 取回 Archive 页面后不再次归档同一正文。Durable Trace 只保存
query、ref、范围、hash 和状态；当前 Agent Loop 仍保留已读取页面，直到正常
Compaction 覆盖它。

## 6. 已接受的限制

- 64 MiB 是单次共享安全捕获硬限。超过部分不能恢复，必须明确标为 Source
  Truncation。
- 搜索 cursor 通过重新扫描和快照指纹保证页间一致性，不持有长期文件系统快照；文件
  变化会使 cursor 失效。
- `workspace_map` 是定位摘要，不保证枚举全部文件；需要更细信息时缩小路径。
- `web_search` 返回 Provider 搜索摘要而非网页全文。Provider 不支持
  `max_output_tokens` 时只能使用其最接近的原生参数；未声明来源完整性时状态保持
  unknown。
- 文档和网页 Provider 解析阶段仍可能需要在内存中形成文本；进程输出已经使用流式
  spool。
- 当前项目没有独立 MCP transport adapter；MCP-style/动态工具通过统一
  `AgentTool`/Extension 入口覆盖。未来增加 transport 时必须复用本契约，不能旁路
  Archive 和 10K Gate。
