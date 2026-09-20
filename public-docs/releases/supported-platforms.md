---
title: 平台与构建状态
description: 查看 Captain Who 当前正式发行的平台、架构和明确的非支持范围。
status: current
audience: user
owner: release-engineering
last_verified: 2026-09-20
---

# 平台与构建状态

“可以从源码打包”与“已经正式发布并支持”是两件不同的事。本页只列出当前用户可以从官网取得、并有完整发行证据的安装包。

| 平台与架构                   | 最低系统版本        | 当前发行状态                   | 安装形式 |
| ---------------------------- | ------------------- | ------------------------------ | -------- |
| macOS Apple Silicon（arm64） | macOS 12 或更高版本 | 正式支持，当前版本为 `1.0.5`   | DMG      |
| macOS Intel（x64）           | —                   | 未正式发布；不在当前支持范围内 | —        |
| Windows（x64 / arm64）       | —                   | 未正式发布；不在当前支持范围内 | —        |
| Linux（x64 / arm64）         | —                   | 未正式发布；不在当前支持范围内 | —        |

当前正式 DMG 为 `Captain-Who-1.0.5-arm64.dmg`。它使用 Developer ID 签名、已通过 Apple 公证，并已装订公证凭据。文件身份和 SHA-256 请以[下载与验证](download-and-verification.md)为准。

## 安装前确认

1. 在 Mac 的“关于本机”中确认芯片显示为 Apple；也可以在终端运行 `uname -m`，预期结果为 `arm64`。
2. 确认系统版本为 macOS 12 或更高版本。
3. 从 [Captain Who 官网](https://captainwhoagent.com/) 下载文件，并核对文件名和 SHA-256。
4. 仅在系统显示的发布者与签名状态正常时继续安装；不要为了安装未知来源文件而关闭 macOS 的安全机制。

## 仍会因平台而变化的能力

即使在受支持的 macOS Apple Silicon 安装包中，个别能力仍依赖本机权限和外部服务。例如，终端与本机 MCP Server 会启动本机进程；系统通知取决于 macOS 通知设置；旧版 `.doc` 文件读取依赖 macOS 系统转换能力，`.ppt` 和 `.xls` 当前不支持。请同时查看[集成兼容性](../integrations/compatibility.md)和[已知问题](../support/known-issues.md)。

## 构建目标不是公开支持

仓库中可能保留其他操作系统或架构的打包入口、运行时资源或测试配置。这些内容用于开发与后续验证，不能视为下载入口、兼容性声明、签名证明或支持承诺。不要通过复制另一平台的应用目录、组件缓存或未签名目录包来安装 Captain Who。
