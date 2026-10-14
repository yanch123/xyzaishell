# 01 自动化验收证据：v0.1.0 候选 1bfaa08

## 1. 结论与精确范围

截至 **2026-10-14 09:27 UTC**，本记录对应的 [CI 第 15 次运行、attempt 1](https://github.com/yanch123/xyzaishell/actions/runs/38540327204) 已完成，**9 个作业全部 success**。运行开始于 `2026-10-14T09:09:51Z`，最终状态更新时间为 `2026-10-14T09:17:29Z`。

- 仓库：`yanch123/xyzaishell`；触发分支：`dev`；事件：`push`。
- 验收源码：[1bfaa08f55392a3679eac30ab8e5cc338c2d64c0](https://github.com/yanch123/xyzaishell/commit/1bfaa08f55392a3679eac30ab8e5cc338c2d64c0)。
- 源码 tree：`14527f360dc86188b0a0a484d76951d21b573511`；应用版本：`0.1.0`。
- 主 workflow：`.github/workflows/ci.yml`；复用的 native acceptance workflow 也解析到上述同一 SHA。
- 结论仅覆盖下列自动化场景。Windows 10/11 实机、真实模型供应商、真实桌面完整交互等仍未验证，详见第 8 节。
- 本文是验收记录，不是发布授权或 release-intent；本次证据核验没有合并 `main`、创建 tag 或发布 Release。后续证据文档提交也不改变这里已测试的源码 SHA。

## 2. 作业与测试结果

所有链接都属于同一个 run `38540327204`，不混用旧阶段的成功结果。

| 作业 | 结果与证据 |
| --- | --- |
| [Frontend tests and production build](https://github.com/yanch123/xyzaishell/actions/runs/38540327204/job/115773196299) | 17 个测试文件、99 个 Vitest 测试通过；TypeScript/Vite 生产构建通过；44 个流水线 Python helper 测试通过 |
| [Browser preview smoke](https://github.com/yanch123/xyzaishell/actions/runs/38540327204/job/115773450892) | 8 个 Playwright 测试通过；覆盖宽/紧凑布局、预览、合成 IPC、任务审批与档案/历史交互；此项使用 mock，不算原生验收 |
| [Windows Rust 与 NSIS](https://github.com/yanch123/xyzaishell/actions/runs/38540327204/job/115773450918) | 普通 Rust 套件 164 个单元测试 + 17 个 SSH 测试通过；fmt、clippy、NSIS 构建通过；3 个锁文件字节/CRLF 回归通过 |
| [Linux Rust 与 DEB](https://github.com/yanch123/xyzaishell/actions/runs/38540327204/job/115773451218) | 普通 Rust 套件 146 个单元 + 15 个模型 + 19 个 SSH + 8 个 agent 测试通过，共 188 个；fmt、clippy、DEB 构建通过；3 个锁文件字节/CRLF 回归通过；显式 OpenSSH 与 Secret Service 运行见下文 |
| [Ubuntu 22.04 安装/启动 smoke](https://github.com/yanch123/xyzaishell/actions/runs/38540327204/job/115775527815) | 实际 DEB 安装、动态链接检查、headless 启动通过 |
| [Ubuntu 24.04 安装/启动 smoke](https://github.com/yanch123/xyzaishell/actions/runs/38540327204/job/115775528043) | 同一 DEB 在 Ubuntu 24.04 实际安装、动态链接检查、headless 启动通过 |
| [Windows Server 2022 原生 UI/生命周期](https://github.com/yanch123/xyzaishell/actions/runs/38540327204/job/115775527933) | 真实安装后的 DOM/IPC、重启、同版本重装、卸载再安装、最终卸载与数据保留检查全部通过 |
| [Ubuntu 24.04 原生 UI/生命周期](https://github.com/yanch123/xyzaishell/actions/runs/38540327204/job/115775527993) | 同上，使用真实 WebKitGTK 与 Xvfb |
| [Ubuntu 22.04 原生 UI/生命周期](https://github.com/yanch123/xyzaishell/actions/runs/38540327204/job/115775528156) | 同上，使用真实 WebKitGTK 与 Xvfb；本轮没有记录到传输重试或连接 reset |

### 2.1 实际 SSH 与凭据提供方

Linux 普通测试中按设计保留 3 个单元测试和 1 个 OpenSSH 集成测试为 ignored；不能将它们计作普通测试通过。随后的专用步骤显式执行：

- **真实 OpenSSH PTY 互操作：1 个测试通过。** `09:12:47.3360431Z` 输出通过；`09:12:47.3667431Z` 独立服务端日志审计确认两次 PTY shell、窗口 resize，以及零 exec/subsystem 请求。fixture 仅使用隔离的临时账户、临时 Ed25519 密钥和 loopback 监听，随后清理。
- **真实 Secret Service：2 个测试通过。** `09:13:01.1871075Z` roundtrip/delete、`09:13:01.4391733Z` locked-provider fail-closed；在隔离的 HOME/XDG/DBus 与合成凭据下执行，不触碰用户 keyring。
- **真实 Windows DPAPI：** `native_dpapi_current_user_roundtrip_and_binding_failure` 于 `09:13:29.0402624Z` 通过。Windows 的 Secret Service 测试因平台不适用而 ignored，不算通过。
- Windows 私有文件 owner/DACL 回归、历史并发 CAS、锁超时、journal 删除生命周期回归通过。具体包括 `access_lock_covers_windows_journal_deletion_lifecycle`、`access_lock_timeout_is_bounded_and_does_not_change_revision`、`concurrent_instances_allow_one_revision_winner`；最后一项包含 16 轮竞争。子进程输出的单独 `1 passed` 不重复计入总测试数。

这些结果证明对应 fixture 的实际操作，不代表已经通过安装后 UI 连接外部 SSH 服务器或读取生产凭据。

## 3. 安装后真实 DOM/IPC 与生命周期

原生测试使用安装出来的可执行文件、真实 Tauri IPC 与真实 Rust HTTP/SSE 路径。模型服务器是仅监听 loopback 的合成 fixture；没有替换生产 IPC，也没有添加生产测试后门。档案在 UI 中保存、重读；历史通过明确同意后保存，重启后必须重新同意，恢复时只恢复文字。

每个平台均有 **3 个 `result.json`、4 次实际应用启动和 6 张截图**：

| 平台 | `first-install-and-restart` | `same-version-reinstall` | `uninstall-and-reinstall` | 最终真实卸载/fixture 清理 |
| --- | --- | --- | --- | --- |
| Windows Server 2022 | 09:17:10.7342277Z 通过 | 09:17:17.3570186Z 通过 | 09:17:24.2623192Z 通过 | 09:17:24.5519667Z 通过 |
| Ubuntu 24.04 | 09:16:43.0722543Z 通过 | 09:16:49.5447353Z 通过 | 09:16:57.2401374Z 通过 | 09:16:58.8180619Z 通过 |
| Ubuntu 22.04 | 09:17:04.9481047Z 通过 | 09:17:12.3744775Z 通过 | 09:17:22.8900673Z 通过 | 09:17:26.1045555Z 通过 |

全部 9 个结果文件均断言：

- `result = passed`、`native_ipc = true`、`synthetic_loopback_model = true`。
- `profile_and_history_persisted = true`、`restored_task_identity_preserved = true`。
- `history_requires_new_process_consent = true`、`restore_model_requests = 0`。
- 恢复后用户/模型消息内容、消息数量与稳定任务 ID 符合保存内容；脚本文字按普通文字显示，没有执行。

三个数据文件 `model.toml`、`connections.toml`、`history.sqlite3` 的 SHA256 在**同版本覆盖安装后、卸载后、再次安装后、最终卸载后**四个边界分别比对成功。Linux 各有 12 条文件 `OK`，Windows 各边界输出配置指纹保持一致。最终卸载使用真实包管理器/NSIS uninstaller，确认程序不存在后才删除测试专用数据目录。不是直接删安装目录冒充卸载。

每次正常关闭先发送真实窗口关闭请求，再核实对应可执行文件的进程已退出，之后才回收 driver；关闭失败会让验收失败，不能用清理时杀进程伪装成功。

### 3.1 Ubuntu 22.04 传输稳定性记录

本轮 Ubuntu 22.04 的 4 份 `driver.log` 中，`reset by peer`、`Error serving connection`、`RemoteDisconnected` 匹配数均为 **0**，没有产生 `read-only-transport-retries.jsonl`，即**记录到的只读重试为 0**。其他两平台也一样；合计 12 份 driver 日志均无上述匹配。

状态变更命令遇到未知传输结果仍直接失败，不自动重放。只读 DOM 查询的有界重试仍要求原 driver 存活且应用 PID 集合与原会话完全一致。此结果是当前精确运行的观测，不能据此证明未来永不发生 WebDriver 波动；此前关于连接池/Connection header 的原因分析仍是待限定的解释，不能把本轮成功倒推成已证实的唯一根因。

### 3.2 实际运行环境和截图

- Windows：**Microsoft Windows Server 2022 Datacenter，10.0.20348**。WebView2 与 Microsoft Edge WebDriver 均为 `131.0.2903.86`。通过进程局部配置选择已安装的对应 runtime；driver 的 Microsoft Authenticode 状态为 `Valid`。这不等于 Windows 10/11 实机结果。
- Ubuntu 22.04：GTK `3.24.33-1ubuntu2.2`，WebKitGTK `2.50.4-0ubuntu0.22.04.1`。
- Ubuntu 24.04：GTK `3.24.41-4ubuntu1.3`，WebKitGTK `2.52.6-0ubuntu0.24.04.1`。
- 三平台使用精确 `tauri-driver 2.1.0`；Linux 为 Xvfb + DBus 测试会话。
- 三平台最终 `uninstall-and-reinstall/preserved/restored-text-no-execution.png` 已目检：中文可读、v0.1 标识可见、用户/模型消息完整、`<script>window.unsafe=true</script>` 保持文字、左右工作区未见遮挡。此目检只覆盖这些截图。
- native artifact 中还包含 `first-install-and-restart/seed/saved-profile.png`、`actual-ipc-recipient-consent.png`、`actual-ipc-streamed-chat.png`，以及首次重启和同版本重装的文字恢复截图。

## 4. 可下载的原始 CI 产物

下列 8 个 ZIP 均已下载并核实 SHA256 与 GitHub artifact 的 `digest` 字段一致；9 个作业日志和三平台 18 张原生截图也已保留。链接可能要求 GitHub 登录。Actions artifact 设置为 14 天保留，本轮显示到期日为 **2026-10-28 UTC**；它们不是永久 Release 附件，发布时需另行保留对应证据和最终产物。

| Artifact | ZIP SHA256 |
| --- | --- |
| [browser-preview-smoke](https://github.com/yanch123/xyzaishell/actions/runs/38540327204/artifacts/11882645664) | `71eebca944cdd062576a7d3d471b712b4e300c306e196021dc5dc73b0ce8120b` |
| [install-smoke-ubuntu-22.04](https://github.com/yanch123/xyzaishell/actions/runs/38540327204/artifacts/11882012994) | `572bb9b476330068310ade9800aaae312a8673732cb19bbfb206b537365c1ee6` |
| [install-smoke-ubuntu-24.04](https://github.com/yanch123/xyzaishell/actions/runs/38540327204/artifacts/11881604237) | `e5c905689c664ee968b4cd5cfb612ee67822e543aa7d0b4769e1163d69d5018d` |
| [installers-linux-x64](https://github.com/yanch123/xyzaishell/actions/runs/38540327204/artifacts/11882511174) | `af2283175dc0ecdc8e4269ccc01d06bedaf4abcc205cc598f7276655843d248d` |
| [installers-windows-x64](https://github.com/yanch123/xyzaishell/actions/runs/38540327204/artifacts/11882346603) | `b4e01d700eab6b193d9d278927a656d1f75fdcad57f4155a5290d948eecebbf4` |
| [native-acceptance-ubuntu-22.04](https://github.com/yanch123/xyzaishell/actions/runs/38540327204/artifacts/11882092591) | `556bac0586aac9dc98f7b98e76e88f16320ffe06bbd86b38eadc51b8cc0e1135` |
| [native-acceptance-ubuntu-24.04](https://github.com/yanch123/xyzaishell/actions/runs/38540327204/artifacts/11882018088) | `7db9a8b3635ba105ce2d749e6d94390fc29fcbf387305b4561695053de0d7016` |
| [native-acceptance-windows-2022](https://github.com/yanch123/xyzaishell/actions/runs/38540327204/artifacts/11881908542) | `0b9f4e5eff7bcb4a4d9395f1b46f3ba30fb63b6b8ca56b22e2577703f103340f` |

## 5. 安装包、源码和 provenance

### 5.1 本轮安装包的准确摘要

下面是 ZIP 内部文件的 SHA256，**与第 4 节 ZIP 自身摘要不同**：

| 文件 | SHA256 |
| --- | --- |
| `xyzaishell-0.1.0-linux-x64.deb` | `3b6b043e5d6c0f2e2a08c157279652030e0d99e4d8d28dfe44a1fce67c66b82a` |
| `xyzaishell-0.1.0-windows-x64.exe` | `d176ca1b3218990250eb2c64b415c66c82b53bb90d3a6807350947c88cf8b49e` |
| `build-linux-x64.json` | `38861e58511a4216cab356317a669cadc66072b788973bdbc39fb47ab745ff37` |
| `build-windows-x64.json` | `d2306cad59e06940721a1b54088e3fc478c9d4fca7afabaa873b6e4eac17968c` |

两份 manifest 均验证了 `source_commit`、`version`、`platform`、`bundle`、预期文件名、安装包摘要与两份锁文件原始字节摘要。各原生安装 wrapper 在安装前也执行该绑定校验，不能拿其他提交产物冒充本 SHA 验收。

| 锁文件 | 两平台共同的原始字节 SHA256 |
| --- | --- |
| `Cargo.lock` | `4a1304d0cbc0075373e5795dfd4a03d8014f977c2a184b123800ddb378492cd3` |
| `package-lock.json` | `f79a5a767dcc11bdee5997a4a3113e91fdf23843a31c974b5aa63ca4f3f7d26a` |

工具链为 Rust/Cargo `1.99.0`、Node `22.23.3`；构建 runner image 分别为 Ubuntu 22.04 `20261004.315.1` 和 Windows 2022 `20261004.326.1`。manifest 记录 `workflow_run = 38540327204`、`workflow_attempt = 1`。以上是内容/来源绑定校验，不是独立签名的供应链证明或可重复二进制构建声明。

同一个安装后应用二进制在各平台全部三个阶段保持以下摘要：

- 两个 Ubuntu 运行环境：`0781bb5ac16896e629c9040e1db26e0042c3b9a59f8516d6173ef4536f42094a`。
- Windows：`fb582d5eea0d6e047ec19a8a7998572b8141442740a883d4db2ddc115120a6c0`。

### 5.2 精确源码快照复核

在本地使用候选自带的 `artifacts.py release` 打包函数，仅生成文件，未调用 GitHub 发布 API：从精确 SHA 创建源码 ZIP/TAR.GZ，并汇总上述两安装包与 manifest 为 `candidate-assets`。两个源码档案的 **164 个普通文件**均逐字节对比该 SHA 的 Git 内容通过；ZIP comment 也等于该 SHA。

| 本地候选源码文件 | SHA256 |
| --- | --- |
| `xyzaishell-0.1.0-source.zip` | `640076a10fe36aeafc6c2ba08afaa66bda94c59ca6793a96f3797e2afab4cd7b` |
| `xyzaishell-0.1.0-source.tar.gz` | `69430078b8d91f1e47014987cf7ec403ae3bf31e9f77ff9399ad3523ee27121c` |

这些源码文件及六项 `SHA256SUMS.txt` 是本地留存的派生产物，不冒充已经上传的 CI artifact 或 Release。之后正式发布若重新构建或重新打包，必须核实其实际来源和摘要并记录新值，不假定二进制会与本轮逐字节相同。

## 6. 已安装的第三方材料

三个原生作业都在**实际安装后的目录**检查下列四项非空并记录摘要；不是只检查源码生成目录或额外 CI 附件。Linux 实际位置为 `/usr/lib/xyzaishell/third-party/`；Windows 为测试安装目录内的 `third-party/`。

| 相对路径 | Linux SHA256（Ubuntu 22/24 一致） | Windows SHA256 |
| --- | --- | --- |
| `THIRD-PARTY-NOTICES.txt` | `906f246763cad4fff1d33b37e10714b0d0d5dd5017b3ca5dc2f88e46c7da48e3` | `7d299f900899b7721afe7f165beaadba18320ef84367f741e796062a9a02f1fe` |
| `RUST-STANDARD-LIBRARY.html` | `5647be074c8edf7339fd863055923a8fc80bc5610a8d4661ec3b767b9d392c27` | 同左 |
| `runtime-inventory.json` | `e6d1ffc98f3e1a202ad69dce38c2aa3074943891957abf43f3d3b1143fbb6ed8` | `2ea80ee27fc57da06c41ac786c2f969c4b116e351b445a2f25adbe3290f58ac1` |
| `sources/option-ext-0.2.0.crate` | `04744f49eae99ab78e0d5c0b603ab218f515ea8cfe5a456d7629ad883a3b6e7d` | 同左 |

源码包摘要与 Cargo.lock 锁定值一致。通知包含目标依赖告知、Rust 标准库告知，以及 Windows 对应 Microsoft WebView2 loader 文本；生成方法和许可核对范围见 [10-release-readiness.md](../10-release-readiness.md)。本节证明材料随安装包可获得，不宣称永久自动完成所有法律判断。

## 7. 证据复核方式

1. 在本记录 SHA 的 checkout 中检查版本和锁文件，下载第 4 节对应平台 artifact。
2. 先核实 ZIP 摘要，再解包，按 `SHA256SUMS-linux-x64.txt` / `SHA256SUMS-windows-x64.txt` 校验内部文件。
3. 运行 `python .github/scripts/artifacts.py verify-package --platform linux-x64 --installers <linux-artifact-dir>`，Windows 使用 `--platform windows-x64`。它校验的是当前 checkout 的精确 SHA、版本、目标、安装包和原始锁文件字节。
4. 对照同平台 native artifact 的三个 `result.json`、四份 `driver.log`、`install.txt`、`installed-notices.sha256` 与截图。最终卸载与配置指纹检查位于 job 日志和 `install.txt` 尾部，不能只读提前写出的 `result.txt` 就认定清理通过。
5. 如选择发布新的构建，完整重跑并保留新来源/摘要；不能将本记录自动套用到其他源码或安装包。

## 8. 尚未验证与发布前需明确的范围

- **Windows 10/11 真实用户桌面尚未验收。** Windows Server 2022 上的自动化结果不能替代它们。
- Ubuntu 22.04/24.04 的实际安装和原生 UI 已在 hosted runner/Xvfb 通过，尚未覆盖真实桌面下的 Wayland/X11 全流程、输入法、剪贴板、辅助功能和完整人机体验。
- 安装后 UI 中的档案场景保存的是合成连接元数据；凭据 provider 的 roundtrip/fail-closed 是独立原生 Rust 测试。没有声称通过 UI 完成外部 SSH 登录、生产凭据读取或真实服务器操作。
- 模型场景使用合成 loopback SSE；真实供应商、真实账户/API key、网络代理及供应商差异未验收。
- **同版本重装已验收，跨版本升级未验收。** v0.1 是首个发布候选，没有已有正式旧版本可用作升级基线。
- 安装包未签名；SmartScreen/未知发布者提示、全新终端首次下载 WebView2 的用户体验、离线安装和不同网络策略未验收。没有自动更新功能验收声明。
- 尚无长时间压力/稳定性保证。本轮传输零异常只是这一组执行结果。
- 是否先提供清楚标明这些限制的预发布试用，或在补齐 Windows 10/11 等验收后正式发布，需要维护者明确决定。本记录不把未完成项改为通过。
