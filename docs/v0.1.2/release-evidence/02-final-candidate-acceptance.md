# v0.1.2 最终候选补充验收

精确源码：`423fc24242ca6b168afc151d7b9f3985a92c1566`；tree `b8a2d38f13fdcd7cfa36acb28727c1841023477d`。
[Build and test 38592011044](https://github.com/yanch123/xyzaishell/actions/runs/38592011044)，attempt 1，11/11 jobs success。记录时间：2026-10-14 UTC。

## 为什么重新验证

上一候选的发布重验运行 38590896759 在 Ubuntu 22.04 的 PTY 中断 fixture 失败，发布步骤未获准执行，不能算发布成功。该 fixture 仅用 `tcgetpgrp != Bash PID` 等待，可能把 PS0 暂时使用的进程组当作已启动前台命令。新候选只修改测试同步及开发说明/索引，未改应用代码。
修正后的真实前台 Python helper 明确设置 SIGINT 默认处理后输出并 flush 唯一 READY 标记；fixture 收到标记后只发送一次 Ctrl-C，严格检查完整 Begin 命令字节、同一 sequence 的 E(130)/R，不重试信号、不放宽成功条件。
既有 01-automated-acceptance 的测试范围及限制仍适用；下面的新构建/原生证据均来自本次精确源码，未将旧安装包 provenance 当成新候选。

## 本次通过项

- 164 前端测试、生产构建、73 helper 测试通过。
- Ubuntu 22.04 Bash 5.1.16 与 Ubuntu 24.04 Bash 5.2.21：真实 OpenSSH 五个 PTY/resize/零 exec 或 subsystem；两边各 12 个当前 shell PTY 用例全部通过，包含修正后的中断用例。
- Linux/Windows workspace tests、fmt、严格 Clippy、安装包构建通过；Linux 隔离 Secret Service 检查通过。
- 浏览器 12 用例通过，21.6 秒；32 PNG。重新查看紧凑终端任务完整目标/命令与设置测试接收方授权画面；合成 mock IPC 限制不变。
- Ubuntu 22.04/24.04 独立安装/链接/Xvfb startup smoke 通过。

| Job | GitHub 结果 |
| --- | --- |
| Frontend tests and production build | [success](https://github.com/yanch123/xyzaishell/actions/runs/38592011044/job/115949031675) |
| Real OpenSSH and current Bash PTY integration (ubuntu-22.04) | [success](https://github.com/yanch123/xyzaishell/actions/runs/38592011044/job/115949521986) |
| Rust tests and linux-x64 package | [success](https://github.com/yanch123/xyzaishell/actions/runs/38592011044/job/115949522023) |
| Browser preview smoke (mock data only) | [success](https://github.com/yanch123/xyzaishell/actions/runs/38592011044/job/115949522139) |
| Real OpenSSH and current Bash PTY integration (ubuntu-24.04) | [success](https://github.com/yanch123/xyzaishell/actions/runs/38592011044/job/115949522177) |
| Rust tests and windows-x64 package | [success](https://github.com/yanch123/xyzaishell/actions/runs/38592011044/job/115949522845) |
| ubuntu-22.04 package install and headless startup smoke | [success](https://github.com/yanch123/xyzaishell/actions/runs/38592011044/job/115953527187) |
| ubuntu-24.04 package install and headless startup smoke | [success](https://github.com/yanch123/xyzaishell/actions/runs/38592011044/job/115953527299) |
| Installed native UI and installer lifecycle / Native UI and installer lifecycle (ubuntu-24.04) | [success](https://github.com/yanch123/xyzaishell/actions/runs/38592011044/job/115953527411) |
| Installed native UI and installer lifecycle / Native UI and installer lifecycle (ubuntu-22.04) | [success](https://github.com/yanch123/xyzaishell/actions/runs/38592011044/job/115953527460) |
| Installed native UI and installer lifecycle / Native UI and installer lifecycle (windows-2022) | [success](https://github.com/yanch123/xyzaishell/actions/runs/38592011044/job/115953527571) |

## 新候选安装包与 provenance

ZIP SHA256 与 GitHub artifact digest 独立匹配；展开后校验安装包与 manifest 的 SHA256SUMS、精确源码/版本/run/attempt 及两个锁文件，全部通过。

| 文件 | 字节数 | SHA256 |
| --- | ---: | --- |
| `build-windows-x64.json` | 1132 | `56229f3acdd8ea480b87230667c220154470e490695eccd6c95abe9f21fd7df6` |
| `xyzaishell-0.1.2-windows-x64.exe` | 6535851 | `66fb1295fc3747e8e96ffa8de2ed84b606f584ecac44df9e75289c4b3345b5ee` |
| `build-linux-x64.json` | 1107 | `4b9e83d94237234619296602ab3006e611c7ea6530e4afbe27b84eaa0bb9e498` |
| `xyzaishell-0.1.2-linux-x64.deb` | 11965332 | `adf22bca6c599a7613150ac9865ff67122314e18b408b86b9042e2d7fdd1bbdf` |

两 manifest 均为 source `423fc24242ca6b168afc151d7b9f3985a92c1566`、version `0.1.2`、workflow `38592011044`、attempt `1`。Cargo.lock 摘要 `c702139185dc0d1dfbcc42e80fb739c036463d40c906912a40b074ff4c99ff70`；package-lock.json 摘要 `7bb5e48e9dd6ba46be18c83877057649dd58ce49f021766f6b40b744ea14c170`。

## 三平台原生生命周期/升级

三个环境各 5/5 result.json passed、27 PNG；所有本次安装 transcript 包含精确候选 SHA，并确认最终卸载后的配置指纹及独立 fixture 清理通过。

| 环境 | 本次已安装程序 SHA256 | 升级结果 |
| --- | --- | --- |
| windows-2022 | `33711c34314c38fc862c51d2ef0e1655456df8c8656215be7cc2bcb5835d4056` | 0.1.1 → 0.1.2 passed |
| ubuntu-22.04 | `22421c99a4dc3984ae32421c35b24bb48f75decf584d16f23a89a47f7dc5d0c9` | 0.1.1 → 0.1.2 passed |
| ubuntu-24.04 | `22421c99a4dc3984ae32421c35b24bb48f75decf584d16f23a89a47f7dc5d0c9` | 0.1.1 → 0.1.2 passed |

每个环境通过本次新装/重启、同版本重装、卸载重装，以及独立 fixture 的已发布摘要固定 v0.1.1 → v0.1.2 实际安装升级。真实原生 DOM/IPC，模型仅合成 loopback。
三个升级 JSON 都确认：旧历史全部行/字段保留并迁移 schema 2、稳定连接/任务 ID 保留、原连接备份字节完全一致及私有权限检查通过、模型文件摘要/高级项保留、恢复只读且要求新进程授权、restore_model_requests=0。Windows ACL 使用 pwsh 且失败即阻止；没有把同版本重装当成跨版本升级。
已重新查看三个环境升级后原生截图，均显示旧合成文本作为只读历史、断开连接且没有重放动作。原生合成元数据升级不证明旧加密凭据或任意旧版本升级。

## Artifact ZIP 摘要

下表 8 个 ZIP 均从本次运行下载后独立计算验证；完整元数据及全部 15 阶段结果随此记录保存。

| Artifact | ID | ZIP SHA256 |
| --- | --- | --- |
| install-smoke-ubuntu-24.04 | 11909260129 | `967ace6a9f0b3fe47cac2e36e3c75a39b4ee1e1c5384902d201ab8011ca969a5` |
| native-acceptance-ubuntu-22.04 | 11908875680 | `d9965cd53d9a021f00ae748eeb6d130c9368f44897f605f699adc42b2f590715` |
| native-acceptance-windows-2022 | 11908755865 | `1d571ddd21d63dde9bbdae0ad000a671b125058a74e341fcca806d3c4f0b1854` |
| installers-windows-x64 | 11908716091 | `5ceeb9e1be8d88d720de72483f9d53d7c4f0e410691032ce40506eeaff4a6191` |
| native-acceptance-ubuntu-24.04 | 11908681883 | `a0ec8d4bb56734a8bf4f42b7669ae46fbab69666661cd363fb2ebc80b38deecb` |
| install-smoke-ubuntu-22.04 | 11908307578 | `21f7ccef0813f8548024f16b3e421262c3864546784e6762e4f50945c5bd3acb` |
| installers-linux-x64 | 11907889255 | `d62e9a36fbbdb3aacc716ad6dbedc8619469bb89e835bca065828d13cfa54a2b` |
| browser-preview-smoke | 11907453752 | `f35840ad12f2a98db73b805bb5e6f905b73d4807725b79d705e13ae90dd2d4d9` |

## 发布边界

该记录支持四个自动化验收类别 passed，Windows 10/11 与 Ubuntu 22.04/24.04 完整桌面四类别继续 pending。仅 prerelease、非 Latest；安装包未签名。外部模型、生产凭据、原生 UI 外部 SSH/非空日志导出全链路、完整 IME/剪贴板/Wayland 和长期压力仍未验收。日志明文/默认关闭/不存输出/不保证未知秘密脱敏/非防篡改审计限制不变。
上述为候选构建摘要；发布流程须再次重建同一精确源码，并校验最终七个公开 Release 资产，不能把候选摘要冒充最终 Release 摘要。
