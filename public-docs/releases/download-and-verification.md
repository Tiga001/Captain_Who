---
title: 下载与验证
description: 从 Captain Who 官网下载 macOS 安装包，并核对发行文件身份与完整性。
status: current
audience: user
owner: release-engineering
last_verified: 2026-09-23
---

# 下载与验证

当前正式发行仅适用于 **macOS 12 或更高版本的 Apple Silicon（arm64）** 设备。请从 [Captain Who 官网](https://captainwhoagent.com/) 开始下载，不要使用搜索结果中的镜像、网盘转存或他人重新打包的文件。

[GitHub 开发仓库](https://github.com/Tiga001/Captain_Who)提供持续更新的源码，不是当前正式安装包的下载入口。仓库中的版本号或源码压缩包不能代替下表中的 DMG。

## 当前发行文件

| 项目           | 值                                                                 |
| -------------- | ------------------------------------------------------------------ |
| 版本           | `1.0.5`                                                            |
| 文件名         | `Captain-Who-1.0.5-arm64.dmg`                                      |
| 文件大小       | 694,547,573 字节                                                   |
| SHA-256        | `5d41e5799a596790e58a18d433fdae03e0cb57285bf7e562cec2155cfbfa7684` |
| macOS 信任状态 | Developer ID 签名，Apple 公证已通过，DMG 已装订公证凭据            |

这组校验信息只适用于上表的 1.0.5 DMG。新版本发布后，请以其对应发行说明中的文件名与校验值为准。

## 下载并核对

1. 在“关于本机”确认芯片为 Apple，并确认 macOS 为 12 或更高版本。
2. 打开官网，下载 `Captain-Who-1.0.5-arm64.dmg`。
3. 在终端进入下载目录，运行以下命令：

```sh
shasum -a 256 Captain-Who-1.0.5-arm64.dmg
```

4. 只有当输出与上表的 SHA-256 **完全一致**时，才继续打开 DMG。
5. 挂载 DMG 后，将 `Captain Who.app` 拖入“应用程序”文件夹，再从该文件夹启动。

如果文件名、文件大小、校验值或 macOS 显示的发布者状态有任何一项不一致，请删除该下载文件并重新从官网下载。不要通过关闭 Gatekeeper、修改系统安全策略或跳过来源警告来安装。

## 关于签名与公证

Developer ID 签名用于让 macOS 识别发布者和安装包是否被篡改；Apple 公证表示该发行包已通过 Apple 的自动化安全检查。装订（stapled）是将公证凭据附加到 DMG，使系统在离线或网络受限时也能验证它。

这些机制不能替代你对下载来源的判断：只有从官网取得且与本页校验信息一致的文件，才是此处描述的正式发行包。

## 使用应用内更新或手动安装

正式签名的 macOS Apple Silicon 版内置由用户发起的应用内自动更新机制，官方公开更新清单现已可用。完整初始化后，只有在可访问的官方更新源提供新版本时，应用才会发现更新；随后由你选择开始下载。下载、完整性校验和原生准备完成后，应用会关闭并重新启动以安装。它不会在你开始下载前自动下载、静默替换或自动降级。更新源暂时不可用时，仍可通过官网 DMG 手动安装，具体步骤见[升级指南](upgrade-guide.md)。

安装步骤见[安装与首次启动](../user/getting-started/installation.md)，平台边界见[平台与构建状态](supported-platforms.md)。如果安装后遇到问题，请先查看[常见问题排查](../support/common-problems.md)和[诊断信息与日志](../support/diagnostics-and-logs.md)。
