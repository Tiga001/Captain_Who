---
title: 版本与发布
description: 查看 Captain Who 的正式安装包、平台范围和对应发行说明。
status: current
audience: user
owner: release-engineering
last_verified: 2026-11-05
release_version: 1.0.7
---

# 版本与发布

## 当前正式版本

Captain Who `1.0.7` 于 2026-11-05 发布，适用于 **macOS 12 或更高版本的 Apple Silicon（arm64）**。请从 [Captain Who 官网](https://captainwhoagent.com/) 开始下载；需要明确指定此版本时，可在[官方 GitHub Release](https://github.com/Tiga001/Captain_Who/releases/tag/v1.0.7) 的 Assets 中选择 `Captain-Who-1.0.7-arm64.dmg`。Source code 压缩包不是安装程序。

- [1.0.7 发行说明](release-notes/1-0-7.md)：了解模型接入、消息预测、组织协作和 Git 审阅等更新。
- [下载与验证](download-and-verification.md)：核对文件名、字节数、SHA-256 和 macOS 安全提示。
- [升级指南](upgrade-guide.md)：使用应用内更新，或验证 DMG 后手动升级。
- [平台与构建状态](supported-platforms.md)：Windows、Intel Mac 和 Linux 尚无正式安装包。项目由个人开发，Windows 版暂无发布时间。
- [历次发行说明](release-notes/README.md)：查看以往版本。

## 主分支不等于安装包

本次发行对应标签 `v1.0.7`（`5dc40325`），已包含此前记录为开发进展的「预测下一条用户消息」、Kimi K3.1、智谱 GLM-5.3 和组织协作等功能。仍使用 `1.0.6` 的用户需要升级，文档更新不会改变旧安装包。[当前开发进展](development-status.md)用于区分后续源码变化与正式发行范围。

应用内更新取决于已安装版本和可用的官方更新源。GitHub 有下载文件，不表示应用内一定会提示同一版本；也不要仅凭版本号或文件名判断两个渠道提供的是同一个文件。
