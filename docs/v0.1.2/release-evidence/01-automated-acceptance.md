# v0.1.2 精确候选自动验收

记录时间：2026-10-14 UTC。该记录是预发布自动化证据，不代表完整人工桌面验收或已发布。

- 源码：`bdd1d1ee9bd3a06cf8eabbcfe8a4ff59c1af192d`
- Git tree：`71422b45a0685dd92b66c4a2ed3f8088654c5b99`
- [Build and test 运行 38588091775](https://github.com/yanch123/xyzaishell/actions/runs/38588091775)，attempt 1，11/11 jobs success。
- 允许的验收结论：`installation_lifecycle`、`ssh_model_security`、`dependency_notices`、`version_upgrade` 自动化范围内 passed；`windows_10`、`windows_11`、`ubuntu_22_04_desktop`、`ubuntu_24_04_desktop` 保持 pending。只作为 prerelease，不作为稳定版或 Latest。
- 发布流程须再次从上述精确源码构建并验证；下面的候选构建摘要不能直接冒充后续 Release 构建摘要。

## 自动测试

| 检查 | 证据 |
| --- | --- |
| 前端与构建 | [job 115935371641](https://github.com/yanch123/xyzaishell/actions/runs/38588091775/job/115935371641)：24 文件、164 测试通过；生产构建通过；73 pipeline helper 测试通过，含 pwsh 备份 ACL 失败关闭回归。 |
| 浏览器 | [job 115935736591](https://github.com/yanch123/xyzaishell/actions/runs/38588091775/job/115935736591)：12 用例通过，24.0 秒；32 PNG，合成 mock IPC。检查宽/紧凑布局、主菜单、连接管理、非模态设置、接收方授权、最小测试、只读历史、终端任务与离线日志。 |
| Linux Rust | [job 115935736342](https://github.com/yanch123/xyzaishell/actions/runs/38588091775/job/115935736342)：workspace tests、fmt、严格 Clippy 和 DEB 构建通过；核心测试 231 passed / 3 ignored，其他集成套件另有通过记录；Linux 隔离 Secret Service roundtrip/delete 与锁定 provider 失败关闭测试单独通过。 |
| Windows Rust | [job 115935736618](https://github.com/yanch123/xyzaishell/actions/runs/38588091775/job/115935736618)：workspace tests、fmt、严格 Clippy 和 NSIS 构建通过；核心测试 240 passed / 1 ignored；`native_dpapi_current_user_roundtrip_and_binding_failure` 实际通过。平台不适用或需独立环境的 ignored 项不被描述成默认套件已执行。 |
| Ubuntu 22.04 OpenSSH/PTy | [job 115935736740](https://github.com/yanch123/xyzaishell/actions/runs/38588091775/job/115935736740)：Bash 5.1.16；真实 OpenSSH 观察到五个 PTY shell、resize、零 exec/subsystem；12 当前 shell PTY 用例通过。 |
| Ubuntu 24.04 OpenSSH/PTy | [job 115935736491](https://github.com/yanch123/xyzaishell/actions/runs/38588091775/job/115935736491)：Bash 5.2.21；同样的 OpenSSH 与 12 当前 shell PTY 用例通过。 |
| Linux 独立安装 smoke | [22.04 job 115938232619](https://github.com/yanch123/xyzaishell/actions/runs/38588091775/job/115938232619)、[24.04 job 115938232685](https://github.com/yanch123/xyzaishell/actions/runs/38588091775/job/115938232685)：DEB 安装、动态链接无缺失、Xvfb 可见窗口且保持运行通过；此项单独不证明交互正确。 |

## 候选安装包与 provenance

从 GitHub Actions 下载 ZIP 后独立计算 SHA256，与 GitHub artifact digest 相同；展开后再核对各 `SHA256SUMS`、manifest、源码 SHA、版本、run/attempt 和两个锁文件。所有检查通过。

| 文件 | 字节数 | SHA256 |
| --- | ---: | --- |
| `xyzaishell-0.1.2-windows-x64.exe` | 6531982 | `0235fdf844756c14c1ac3b6e838b5f94e6e5bf547c2aa5f020a1e0e06d7f6a1d` |
| `build-windows-x64.json` | 1132 | `465168747c1d5778bd7fea7dc638da38915c2e0a33464794612ff100b8cc554e` |
| `xyzaishell-0.1.2-linux-x64.deb` | 11965344 | `f7b40ee8d320255f1aeb403580558faf77ee9cd8722983dd4b860f1b7693f780` |
| `build-linux-x64.json` | 1107 | `16a9b43d6dcc9532c70d71816f8aa5daf1cca72ac76ca9e44361340c972bf3f1` |

两 manifest 均为 version `0.1.2`、source `bdd1d1ee9bd3a06cf8eabbcfe8a4ff59c1af192d`、workflow `38588091775`、attempt `1`，Rust/Cargo 1.99.0、Node v22.23.3。

- `Cargo.lock`：`c702139185dc0d1dfbcc42e80fb739c036463d40c906912a40b074ff4c99ff70`
- `package-lock.json`：`7bb5e48e9dd6ba46be18c83877057649dd58ce49f021766f6b40b744ea14c170`
- 构建生成第三方与 Rust 标准库告知、适用源码，并核对已审阅的 WebView2 loader 哈希与告知文本。原生安装阶段核对 aggregate 告知、runtime inventory、标准库告知和 `option-ext-0.2.0.crate` 均存在、非空并记录 SHA256；不额外声称逐个已安装 loader 文件的检查。

## 真实安装应用生命周期与跨版本升级

| 环境 | job | 阶段结果 | 已安装 v0.1.2 程序 SHA256 |
| --- | --- | --- | --- |
| Windows Server 2022 | [115938232722](https://github.com/yanch123/xyzaishell/actions/runs/38588091775/job/115938232722) | 5/5 result.json passed，27 PNG | `f4a4702ed4ac545018176de5e05cbc50991215954e76bedefdb5e9baecd751de` |
| Ubuntu 22.04 / Xvfb | [115938233040](https://github.com/yanch123/xyzaishell/actions/runs/38588091775/job/115938233040) | 5/5 result.json passed，27 PNG | `22421c99a4dc3984ae32421c35b24bb48f75decf584d16f23a89a47f7dc5d0c9` |
| Ubuntu 24.04 / Xvfb | [115938233105](https://github.com/yanch123/xyzaishell/actions/runs/38588091775/job/115938233105) | 5/5 result.json passed，27 PNG | `22421c99a4dc3984ae32421c35b24bb48f75decf584d16f23a89a47f7dc5d0c9` |

每个环境分别完成：v0.1.2 首次安装/进程重启、同版本重装、卸载后重装；另一个隔离 fixture 安装已发布且摘要固定的 v0.1.1、通过旧版真实 UI 建立非秘密连接档案及合成聊天并重启，再直接安装 v0.1.2。所有阶段使用真实原生 DOM/IPC，模型仅为独立 loopback fixture。

- 新装阶段真实 UI 保存模型设置，另行明确批准固定最小模型测试；保存不自动发送。合成流式文本和类似脚本的文字均作为文字显示。
- 升级前后版本确为 0.1.1 → 0.1.2，安装程序哈希不同；Linux 旧程序 SHA256 `c791c94868c68ff280a279170c3ec70a0e352a152d727e638b40f78b330e931b`，Windows 旧程序 `369b98e1fab0c1ff8b3d8cea88224249b121914945f65dc21cdf19f50817f013`。
- 升级安装结束、首次新版本启动前，模型/连接/历史数据文件摘要保持不变。启动后历史 schema marker 为 2，每一条旧历史的 chat/task ID、revision、listed、timestamp 和 payload 原样保留。允许产生无关的新空会话，不以数据库文件字节相同代替迁移断言。
- 连接稳定 ID/目标保持；`connections.v1.backup.toml` 与旧连接文件字节摘要完全相同。Linux 当前用户属主 + 0600；Windows 当前用户属主、受保护 ACL、仅当前用户/System/Administrators 的允许项，由 pwsh 校验，任意失败阻止 passed。
- 模型配置文件 SHA256 和五个高级选项保持；正常关闭后没有 SQLite WAL/SHM/journal 残留。
- 三平台升级 result 均含 `legacy_history_rows_and_protected_profile_backup_verified=true`、`profile_id_preserved=true`、`restored_task_identity_preserved=true`、`history_requires_new_process_consent=true`、`restore_model_requests=0`。
- 历史恢复为只读、不会自动建立 SSH 或模型请求；无日志时导出失败关闭。最终卸载移除应用且保留配置指纹，再删除独立 fixture，三平台 transcript 均确认清理通过。
- 已逐一查看三平台升级后的只读历史截图，以及三平台模型设置保存状态截图；浏览器另查看紧凑任务授权/已发送说明、设置授权、菜单、连接管理和日志。截图反映被捕获的滚动位置，不声称滚动区域的所有控件同时可见。

原生驱动：tauri-driver 2.1.0；Windows WebView2 与 Microsoft Edge WebDriver 均为 131.0.2903.86，driver Authenticode 为 Microsoft Corporation/Valid，测试进程锁定该已安装 runtime。Windows driver provenance 中 SHA256 仅证明下载字节，真实性检查使用 Authenticode，不冒充供应商 checksum manifest。Linux runtime：22.04 WebKitGTK 2.50.4、GTK 3.24.33；24.04 WebKitGTK 2.52.6、GTK 3.24.41。

## Artifact ZIP 摘要

所有下列 ZIP 均已下载且独立验证，均来自相同精确候选/run；GitHub 保留期至 2026-10-28，永久文档须保存必要结果摘要/截图。

| artifact | ID | ZIP SHA256 |
| --- | --- | --- |
| browser-preview-smoke | 11905344326 | `684562a032e6e6916aa0a5f4bb1706b0405b16ca94313d0b8a0d267aa71d993a` |
| installers-windows-x64 | 11906861184 | `381a44848947798ec6b5853781aa30d9598f6ff3edc8a0a68d748c564c4a94fc` |
| installers-linux-x64 | 11906501943 | `d3f6235c7cd6940eb7d8681ced703d4c1fa1d35e527ea3d6804ea0a501056efb` |
| native-acceptance-windows-2022 | 11907035544 | `c9a06a38b2cf12bc126d3913e51f3deac2c462bde9ee46081112b98fb822e3a5` |
| native-acceptance-ubuntu-22.04 | 11907590189 | `e330ac6734eb497fcc510e85a623897fbde5d35a941b1376f7df26069b8e0b19` |
| native-acceptance-ubuntu-24.04 | 11907715053 | `80d47587c77738067e6713b89ecda40a7d1d5ca40a70138cc8bb904754222153` |
| install-smoke-ubuntu-22.04 | 11905954784 | `c83672f7d4c18450712972a781e4f4e187d361cffce49a65515180dc64efa93b` |
| install-smoke-ubuntu-24.04 | 11906099757 | `1b0ec0df4baf386ddc2913d5ac11607ea490efc779c3b8a79c3450432262ebdd` |

## 明确保留的限制

安装包未签名。Windows Server 2022 不替代 Windows 10/11；Xvfb 不替代 Ubuntu 完整用户桌面、X11/Wayland、输入法、剪贴板、辅助功能或长期压力验收。未测试真实外部模型供应商、生产凭据、安装后 UI 连接外部 SSH 并导出非空日志的完整链路。真实 OpenSSH 集成与 provider 单测范围独立，不混为 UI 链路。该旧版合成元数据升级不证明任意历史版本或旧加密凭据迁移。

日志默认关闭，启用后为可选明文，不保存终端输出、不自动发给模型，不保证未知秘密均被脱敏，也不是不可篡改审计。原生新装 `result.txt` 的“未测 version upgrade”仅指该阶段，跨版本证据来自独立 `version-upgrade` 阶段，不能省略该区分。
