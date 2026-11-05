---
title: 下载与验证
description: 获取 macOS Apple Silicon 的官方 DMG，核对下载文件，再交由 macOS 检查。
status: current
audience: user
owner: release-engineering
last_verified: 2026-11-05
---

# 下载与验证

先在「关于本机」确认芯片是 Apple、系统为 macOS 12 或更高版本。可以从 [Captain Who 官网](https://captainwhoagent.com/) 开始下载；要取得下表中的确切版本，请到[项目官方 GitHub Release](https://github.com/Tiga001/Captain_Who/releases/tag/v1.0.7) 的 **Assets** 下载 DMG。不要选择 Source code，也不要用网盘转发或第三方重打包文件。

## 当前发行文件

| 项目    | 值                                                                 |
| ------- | ------------------------------------------------------------------ |
| 版本    | `1.0.7`（2026-11-05）                                              |
| 文件名  | `Captain-Who-1.0.7-arm64.dmg`                                      |
| 大小    | 697,208,033 字节                                                   |
| SHA-256 | `017c3d5f709c1735df53d1c51e0f01ceedcfc0af12f2536f941fd8535cfa4bf7` |

大小和摘要对应 [GitHub 公开发行附件](https://github.com/Tiga001/Captain_Who/releases/expanded_assets/v1.0.7)，并已与随包发布的 [SHA256SUMS.txt](https://github.com/Tiga001/Captain_Who/releases/download/v1.0.7/SHA256SUMS.txt) 核对。从其他官方入口下载时，也请对照实际拿到的文件：若版本不同，请使用对应[发行说明](release-notes/README.md)，不要套用上表的摘要。

## 下载并核对

在终端进入下载目录，运行：

```sh
shasum -a 256 Captain-Who-1.0.7-arm64.dmg
```

1. 逐位比较输出的 64 个十六进制字符与上表；不同就停止安装，重新从官方发行页下载。
2. 摘要相同后打开 DMG，把 `Captain Who.app` 拖入「应用程序」，再从该文件夹启动。

## 关于签名与公证

摘要一致只表示与发行附件的内容相符，不能替代 macOS 的签名和公证检查。[官方发行记录](https://github.com/Tiga001/Captain_Who/releases/tag/v1.0.7)说明 `1.0.7` 的 macOS 安装包已完成 Developer ID 签名、Apple 公证及公证票据验证；本页核对的是该记录和发行附件摘要，未重新执行安装包验签。若系统报告来源异常、签名无效或文件被修改，请停止安装，不要关闭 Gatekeeper 或绕过提示。

## 使用应用内更新或手动安装

旧版 DMG 的校验值和公证记录只适用于相应文件，见 [1.0.6](release-notes/1-0-6.md) 和 [1.0.5](release-notes/1-0-5.md) 发行说明。已有安装的用户请先看[升级指南](upgrade-guide.md)，首次使用请看[安装与首次启动](../user/getting-started/installation.md)。
