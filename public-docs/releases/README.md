---
title: 版本与发布
description: 查看 Captain Who 的正式安装包、平台范围和对应发行说明。
status: current
audience: user
owner: release-engineering
last_verified: 2026-11-03
release_version: 1.0.6
---

# 版本与发布

## 当前正式版本

Captain Who `1.0.6` 于 2026-09-23 发布，适用于 **macOS 12 或更高版本的 Apple Silicon（arm64）**。请从 [Captain Who 官网](https://captainwhoagent.com/) 开始下载；需要明确指定此版本时，可在[官方 GitHub Release](https://github.com/Tiga001/Captain_Who/releases/tag/v1.0.6) 的 Assets 中选择 `Captain-Who-1.0.6-arm64.dmg`。Source code 压缩包不是安装程序。

- [1.0.6 发行说明](release-notes/1-0-6.md)：了解这个安装包包含的功能。
- [下载与验证](download-and-verification.md)：核对文件名、字节数、SHA-256 和 macOS 安全提示。
- [升级指南](upgrade-guide.md)：使用应用内更新，或验证 DMG 后手动升级。
- [平台与构建状态](supported-platforms.md)：Windows、Intel Mac 和 Linux 尚无正式安装包。项目由个人开发，Windows 版暂无发布时间。
- [历次发行说明](release-notes/README.md)：查看以往版本。

## 主分支不等于安装包

9 月 23 日的发行文件对应标签 `v1.0.6`。11 月 3 日的主分支仍用同一版本号，却已加入「预测下一条用户消息」等后续功能；它们不会因为官网文档更新就出现在已安装的软件里。[当前开发进展](development-status.md)单独记录这些变化。

应用内更新取决于已安装版本和可用的官方更新源。GitHub 有下载文件，不表示应用内一定会提示同一版本；也不要仅凭版本号或文件名判断两个渠道提供的是同一个文件。
