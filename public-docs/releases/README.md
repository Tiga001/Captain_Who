---
title: 版本与发布
description: 获取 Captain Who 正式版本、核验下载文件并了解升级方式。
status: current
audience: user
owner: release-engineering
last_verified: 2026-09-14
---

# 版本与发布

本区域记录已经正式发布、并且可以由用户核验的 Captain Who 安装包。源码中的版本号、开发构建或打包配置本身，不等于某个平台已经对外发行。

## 当前正式版本

Captain Who `1.0.2` 于 2026-09-14 发布。目前唯一正式支持的安装包为 **macOS 12 或更高版本的 Apple Silicon（arm64）** 版：`Captain-Who-1.0.2-arm64.dmg`。

该 DMG 已使用 Developer ID 签名、通过 Apple 公证，并已装订（stapled）公证凭据。请始终从 [Captain Who 官网](https://captainwhoagent.com/) 获取安装包；不要把聊天群、网盘或第三方重打包文件当作官方发行。

Windows、Intel Mac 和 Linux 目前没有正式安装包，也不在当前公开支持范围内。它们即使出现在源码的构建配置中，也不代表可以下载安装或获得兼容性承诺。

正式签名的 macOS Apple Silicon 发行版内置应用内自动更新机制。必须先将应用安装到本机并从已安装位置启动；直接从挂载的 DMG 或 macOS App Translocation 路径运行时不会启用更新。完整初始化后，只有在可访问的官方更新源提供新版本时，应用才会发现更新；随后由你选择开始下载。下载、完整性校验和原生准备完成后，应用会关闭并重新启动以安装更新。它不会在你开始下载前自动下载、静默替换或强制更新，也不会自动降级。官网 DMG 始终是核验与手动升级的备用方式。

## 文档

- [下载与验证](download-and-verification.md)：确认平台、文件名、文件大小、SHA-256 和 macOS 信任状态。
- [平台与构建状态](supported-platforms.md)：了解当前正式支持的平台边界。
- [升级指南](upgrade-guide.md)：使用应用内更新，或通过官网 DMG 手动升级。
- [发行说明](release-notes/README.md)：查看每个已经正式发布版本的用户可见变更。

## 版本信息的使用方式

发布页面、安装包文件名、应用“关于”页面和本区域的版本应相互对应。若它们不一致，先停止安装或替换操作，并重新从官网取得文件。校验值只能验证下载文件是否与本页列出的发行文件一致，不能让第三方重打包文件变成官方版本。
