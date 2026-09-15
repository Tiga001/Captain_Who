---
status: current
audience: developers/maintainers
owner: engineering
last_verified: 2026-09-16
---

# 桌面应用更新

桌面更新使用 `electron-updater` 与 Electron Builder 的 generic HTTPS 源。当前范围为 macOS arm64；Windows、Linux、macOS x64/universal 和 Mac App Store 不启用更新发布。源码实现和配置测试不代表真实发行链路已经验收。

## 应用行为与安全退出

依赖固定为 `electron-updater@6.8.9`，使用现有 Electron 39 与 Builder 26。主进程在 Host 初始化完成后每次启动检查一次，不自动下载、不轮询。开发模式、缺更新配置、非 macOS arm64、从 DMG 或 App Translocation 路径启动时禁用自动更新。

左下角账号栏右端是紧凑的更新插槽：没有可显示的更新动作时（检查中、已是最新或更新被禁用）显示主进程拥有的帮助菜单（“关于 Captain Who”与“查看文档”；不接收渲染进程传入的 URL 或原生面板参数，出现更新控件时让位）。发现新版本时插槽为环形图标按钮（文字只作为无障碍名称与提示）；进入下载、校验或重启阶段后改为胶囊文字，只显示主进程报告的阶段与整数进度：“正在下载 XX%”→“校验更新”→“正在重启”。下载或安装准备失败时保留可重试按钮，显示简短错误，不影响正在运行的应用。没有新增设置页、确认弹窗、发布说明页。

`UpdateService` 管理 `disabled/checking/idle/available/downloading/preparing/installing/error` 状态；preload 只暴露无参数的 `getState()`、`download()` 与 `onStateChanged()`。状态只有 revision、版本、百分比、状态及固定错误码，不包含账号、地址或原始错误。渲染进程不能指定 URL、安装路径、命令或直接要求退出安装；重复下载请求合并，过期状态不能覆盖新状态。

第一版使用完整 ZIP 下载，并且只接受包含唯一 arm64 ZIP 的清单。electron-updater 的 ZIP-ready 事件早于 Squirrel.Mac 验证完成；因此关闭 SDK 的 `autoInstallOnAppQuit`，显式进行原生安装准备。ZIP 传输到 100%（包括已验证的缓存 ZIP）只表示传输完成，状态进入 `preparing`（“校验更新”）；只有原生 `update-downloaded` 成功后才进入 `installing`（“正在重启”）并请求自动安全退出，不能把“下载完成”当作“可安装”。原生明确报错才恢复重试；没有人为超时后假定原生操作已经取消的逻辑。原生准备期间百分比可以暂留 100%，应用仍可继续使用。

安装复用 `AppShutdownCoordinator` 的唯一退出事务：通知和新输入停止 → renderer 有界持久化 flush → Core Server、终端等既有有界停机 → 受管浏览器/桥接清理 → 放行退出 → 调用 installer。普通退出已开始时拒绝额外的安装派发；清理和安装各执行一次，不循环触发退出。运行中任务遵循现有停机策略，不承诺自动恢复全部任务。

必须区分“SDK 缓存了 ZIP”与“原生 Squirrel 已开始准备”。仅检查版本或缓存 ZIP 不会触发原生安装；用户点击下载并进入原生准备后，该阶段不可真正取消。若用户此时普通退出，仍先完成原有安全清理，但系统可能在退出后/下次启动应用已经准备好的更新。关闭 SDK 的 `autoInstallOnAppQuit`、取消网络 token 或移除 JS 监听都不能撤销原生准备；不能向用户承诺一定保持旧版。本版不提供取消按钮，点击下载代表同意随后自动安装。[Electron 39 官方语义](https://raw.githubusercontent.com/electron/electron/v39.8.10/docs/api/auto-updater.md)

如果安装调度在服务已停止后同步或异步失败，仅请求重启当前应用一次，不把已经停止的旧窗口重新展示成可用状态。此时实际系统安装行为仍须 A → B 真实验收。更新不删除或迁移账号、聊天、模型配置、SQLite 或用户数据目录。

下载使用 SDK 独立的非持久化网络 session，并限制到构建配置的同一 HTTPS origin 和目录（包括重定向）。请求头剥离账号授权、Cookie、Referer、代理授权及 SDK 默认 `x-user-staging-id`，不新增设备标识上报。COS 必然能观察 HTTP 请求元数据；这不代表零网络日志。

## 更新源与构建契约

`CAPTAIN_WHO_UPDATE_URL` 是**构建时**注入的更新目录 HTTPS URL。地址不写死在源代码、renderer 配置或第二份 runtime JSON 中。Electron Builder 将验证后的 generic provider 写入最终应用的 `Contents/Resources/app-update.yml`，运行时只以此文件为更新源；后续改变 URL 必须重新构建应用。

地址必须是长期可读的 HTTPS 目录，不能带 user/password、query、fragment、反斜杠或空白。临时签名 URL 不适合作为更新源。COS SecretId/SecretKey、上传 token 和可写凭据不得进入应用、更新 URL 或 renderer；上传者的凭据仅属于后续独立发布流程。

构建配置分为两层：

- `electron-builder.yml`：基础签名、冻结组件与打包真源；默认 `publish: null`、DMG-only，未配源的开发构建可正常使用，更新禁用。
- `scripts/update-config.mjs`：读取基础 YAML 并验证构建环境；有合法 URL 时启用 `mac.publish` 的单个 generic/latest provider、arm64 DMG 与 ZIP、DMG update info 和强制签名。Windows/Linux 和目标级 provider 仍禁用。`beforePack` 按真实目标拒绝其他 macOS 架构、MAS、缺 ZIP 或显式禁用签名。

不执行构建即可检查源配置：

```bash
pnpm verify:update-config
pnpm test:update-config
```

`verify:update-config` 要求 URL 存在。`build:mac` 在组件准备前调用该检查，因此正式可更新构建缺少地址会提前失败。独立调用配置时可设置 `CAPTAIN_WHO_REQUIRE_UPDATES=1` 取得相同的缺源失败语义；`node scripts/update-config.mjs` 则允许无源开发配置。检查只验证语法与配置，不访问 COS、不证明服务器可读、不执行签名或打包。

后续获得正式地址、证书和发布条件后，构建进程从外部环境取得 `CAPTAIN_WHO_UPDATE_URL` 再执行 `pnpm build:mac`。命令显式使用 `--publish never`，产物生成不会自动上传。generic provider 本身也不实现 COS 上传。

有源 macOS arm64 构建的预期产物为：

| 产物                                    | 用途                                                        |
| --------------------------------------- | ----------------------------------------------------------- |
| `Captain-Who-${version}-arm64.dmg`      | 首次安装                                                    |
| `Captain-Who-${version}-arm64.zip`      | macOS updater 安装包                                        |
| `latest-mac.yml`                        | latest 通道版本与下载文件摘要                               |
| Electron Builder 生成的对应 `.blockmap` | 下载辅助文件；与本次归档一起保存、核对和发布                |
| app 内 `app-update.yml`                 | 唯一 runtime provider 与 URL，不作为另一个远端通道 manifest |

`afterPack` 与 `afterSign` 均读取**最终 builder 配置**，验证 `app-update.yml` 是普通文件、provider/URL/channel 与构建配置完全相同，且没有附加凭据字段。无源或不支持的平台拒绝包中出现该文件。原有 app、Core Server、冻结 Mach-O receipt、签名和隐私校验继续执行。

## 签名、公证与最终字节

`build:mac` 和有源动态配置要求真实 Developer ID 签名。现有 `scripts/sign-macos.mjs` 自定义签名、稳定 Core Server identifier、同 Team ID、hardened runtime 和冻结 receipt 契约保持有效，不能为了更新跳过或放宽。

`notarize: false` 仍明确保留。当前没有完成自动 notarization/stapling 集成，也没有执行本轮真实签名、公证、打包或上传。公证凭据与 Apple 操作属于后续需要发布者参与的步骤。

正式上传前必须完成公证、必要 stapling 与最终验签，然后冻结真正分发的 ZIP/DMG。若公证或 stapling 改变了 app/归档字节，必须重新生成包含最终 app 的归档、blockmap 和对应 manifest 摘要；不能继续使用字节变化前的 `latest-mac.yml`，也不能手改摘要掩盖未完成的构建验收。

## COS 分发与发布顺序

更新目录需要支持匿名 HTTPS 读取**具体对象**。公开对象读不等于开启桶列目录权限，客户端不需要 ListBucket、列出全部对象或任何写权限。现有其他业务对象、bucket policy 与权限设置不在应用侧实现中自动修改。

正式发布顺序：

1. 确认单独的 macOS arm64 更新目录、HTTPS 访问策略、范围读取和缓存策略；维护者独立保管上传凭据。
2. 在同一最终源码树完成 package gate、真实签名、公证、stapling、最终 app/DMG 验签与启动验证。记录版本、commit、大小和摘要；验证 ZIP 内的真实 bundle 版本与 manifest 一致。`allowDowngrade=false` 只验证 manifest 版本，不能发现高版本 manifest 错指历史签名包。
3. 先上传版本化 ZIP、DMG 及所有对应 blockmap；版本化归档不能在发布后悄悄覆盖。
4. 按将写入 manifest 的准确 URL 下载或校验远端对象，核对大小与 SHA-512 和本地最终字节一致；需要范围读取时验证响应。HEAD/200 或能看到桶中的文件名不能代替内容校验。
5. **最后**上传 `latest-mac.yml`，使其仅引用已就绪且验证通过的对象。manifest 使用短缓存或重新验证策略；版本化归档可使用长缓存。需缓存刷新时先确认新对象可读取，再切换 manifest。
6. 从安装的旧版本走真实更新验收，并归档服务器请求、版本变化、错误状态和应用数据检查结果。

manifest 最后发布可避免客户端看到尚未上传完毕的包；不能把“上传成功”当作签名、公证、安装或数据兼容证据。

## A → B 验收与回退边界

后续真实验收暂定 A=`1.0.0`、B=`1.0.1`。本轮没有为此修改版本、生成包或操作日常使用的应用。两版需要相同 appId、有效且兼容的签名身份、相同数据根语义和可读取同一更新目录的配置。

在隔离测试环境安装 A，准备可核对的设置、会话和任务数据，再发布 B。验证检查更新、发现新版本、下载进度、错误提示、下载与原生准备完成后自动安全退出，以及重启后版本为 B；同时验证仅检查不会自动下载/安装、原生准备期间普通退出仍先安全清理、活动任务处理和原有数据保留。首次安装 DMG 与 updater 使用的 ZIP 都要来自本次最终验收产物。

失败路径至少包含：无源开发构建、不支持平台、404/断网、无新版本、重复点击或并发请求、普通退出取消下载、失败重试、下载摘要错误、未就绪安装及安装调度失败。用可控 updater 替身验证这些分支只能证明应用逻辑；macOS 原生 updater、真实签名、网络对象和重启替换必须使用真实 A → B 测试。

更新安装不运行开发用 storage reset。应用不提供自动降级和数据库回滚；若需停止坏版本，应停止该版本分发并发布更高版本修复，不能假设把 manifest 指回低版本会自动回退。每次涉及 schema 的更新都需独立评估旧数据兼容与恢复方案。

## 尚未完成

- 正式 COS 更新目录、对象匿名读取/范围读取/缓存配置及其线上证据。
- 发布上传实现或实际上传，包含 manifest 最后发布与远端最终字节核对。
- Apple notarization/stapling 集成和真实证书、公证、最终签名验收。
- 最终 macOS arm64 DMG/ZIP/manifest 的真实构建和 package gate。
- 真实 A=`1.0.0` → B=`1.0.1` 下载、重启替换、失败恢复与数据保留验收。
- Windows/Linux/macOS 其他架构的更新发布支持。

完整发布条件与现有签名证据边界见[构建与发布](../development/build-and-release.md)。
