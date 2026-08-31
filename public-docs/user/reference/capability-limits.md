---
title: 能力限制
description: 汇总当前版本面向用户的重要支持范围、上限和不适用场景。
status: current
audience: user
owner: product-docs
last_verified: 2026-08-31
---

# 能力限制

本页用于避免把“行业中可能存在的能力”误当成当前 MyCopilot 已支持功能。数值是安全上限，不是建议把每次任务都用到上限。

## 模型与网络

- 模型接入支持通用 OpenAI-compatible、通用 Anthropic-compatible 和 DeepSeek V4 Chat 配置；兼容端点仍需满足对应协议行为。
- 联网搜索依赖用户配置的 Tavily API Key；没有 Key 时不能使用搜索 Tool。
- 模型与搜索不是离线能力，数据会发往相应配置服务。
- Token 和费用是本地统计或估算，不代表服务商最终账单。

## 项目、文件与附件

- 输入框选择的单个附件最大为 8 MiB。向正在运行的 Agent 补充消息时，一条消息最多携带 8 个附件、合计 32 MiB。
- Agent 附件支持文本、图片、PDF、`.docx`、`.pptx`、`.xlsx`、`.csv`、`.tsv` 等当前登记格式。
- 旧 `.doc` 只在 macOS 通过系统能力解析；旧 `.ppt` 和 `.xls` 暂不支持。
- PDF 附件不会在发送时自动提取正文；需要激活 PDF Skill 后按需提取或渲染。扫描 PDF 没有文本层时，应通过页面渲染和视觉能力检查。
- 文件 Tool 的能力与右侧 Files 预览不同。Files 预览是只读的，不支持编辑、创建、重命名或删除。
- Files 文本预览最多 1 MiB/5,000 行，图片最多 12 MiB，PDF 最多 32 MiB；Office 文件不能直接在 Files 面板预览。
- Symlink 可以在文件树出现，但不能展开、预览、复制真实路径或在文件管理器中定位。

## 文件修改与命令

- 结构化文件写入和补丁受工作区、文件版本、父目录和 symlink 检查；自动批准不会跳过这些校验。
- 结构化文件创建、更新和删除统一使用 FileChange；更新/删除必须绑定先前读取到的文件版本，冲突时会要求重新读取。命令或 Office 工作流产生的文件副作用使用各自的权限和审批边界。
- 单个 FileChange 事务有内容和 Diff 上限；大文件会使用同一事务的分段写入，超限时应缩小单次目标。
- 命令会按完整语句和风险分类；完全权限仍不能执行始终拒绝的操作。
- 长命令使用受管 Command Session；取消不总能证明外部副作用没有发生。

## Skill

- Skill 来源为 Bundled、Installed 和当前项目 Workspace；入口文件必须精确命名为 `SKILL.md`。
- 默认一次 Run 最多激活 8 个 Skill，激活说明源码合计最多 512 KiB；超大目录可能被裁剪并显示诊断。
- Workspace Skill 只在当前项目发现，新建或修改后要在新的 Run 中重新发现。
- 用户安装主要支持 GitHub 和已授权本地目录。
- Skill Script 当前只支持 `scripts/*.py` 和宿主 Python 3；缺失依赖不会自动安装。预检和执行需要高权限条件，Installed/Workspace 脚本始终需审批；只有重新验证的应用内置脚本可在满足全部条件时使用内置执行自动批准。

## MCP 与浏览器

- 用户 MCP Server 只支持本地 stdio；HTTP、Streamable HTTP、OAuth、环境变量、SecretRef 和自定义 headers 尚未开放。
- Agent 产品路径目前只使用 MCP Tools；Resources、Prompts、Sampling、Elicitation、Tasks 和 MCP Apps 未开放。
- MCP 调用超时、取消或连接丢失后可能成为结果未知，不能自动重放有副作用的调用。
- 内置浏览器支持手动浏览和受管 Agent 自动化；网页申请的摄像头、麦克风、定位和通知等权限默认拒绝。
- 浏览器 Session 跨应用会话保留 Cookie 与站点存储，直到用户执行“清除浏览数据”。
- 浏览器下载会保存到配置目录并登记持久历史，单个 Agent 下载安全投影最大 2 GiB；历史可用性只检查缺失、非普通文件或大小变化，不检测同尺寸内容替换。
- 截图等非下载 Browser Artifact 仍是临时 Run 资源，需要长期保留时应显式导出。

## Multi-Agent

- 协作结构是父子树，不是通用 DAG、图形工作流或自动规划器。
- 用户只与根 Agent 交互；子 Agent 对话只能只读观察。
- 默认树深度上限 8、每棵树最多 64 个节点；全局同时执行的 Agent Turn 默认最多 4 个。
- 同一 Agent 同时最多一个活跃 Turn。模板存于工作区库并显式分配项目，修改只影响未来节点。
- 更多节点不会线性提升速度，也会增加模型调用和上下文成本。

## Scheduled Automation

- 调度器只在应用和 Core Server 运行时工作，不是系统后台服务，也不承诺秒级准点。
- 支持固定分钟/小时/天间隔，以及每天、工作日、每周和按小时/日/周/月/年的自定义重复；不支持 cron、一次性计划和结束日期。
- 离线错过的多个计划点会合并成一次恢复 Run，不逐次补跑。
- 同一 Task 一次最多一个未结束的 Run；多个 Scheduled Automation 仍与普通根 Agent 和子 Agent 共享全局执行容量。
- 当前 UI 不能单独停止活动 Run。暂停只影响未来计划；删除会请求取消。
- 没有独立的审批超时、运行历史保留或清理设置；原生通知失败时也没有应用内提醒替代。

## Office、Artifact 与图片生成

- Word、表格和演示文稿的新建/编辑依赖对应 Bundled Skill 和受管构建流程；基础 Office Tool 主要负责检查、验证与渲染。
- 通用受管 Artifact 当前主要支持 PNG、JPEG、WebP 和 PDF；Office 文件仍作为工作区文件交付。
- 图片生成当前只有一个内置的 Seedream 配置档，默认 `2K` 预设，不支持任意宽高协议。
- 图片生成请求受并发和队列上限保护；可能已经产生费用的未知结果不会自动重试。

遇到限制时，优先缩小任务、拆分输入或使用明确的导出/保存步骤，不要通过扩大权限绕过资源和安全上限。
