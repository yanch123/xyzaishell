# 01 — v0.1.1 自动验收记录

## 1. 精确候选与结论

- 验收候选：[`18eba0cf02a2c265862e957ba86f1cb24e4bedde`](https://github.com/yanch123/xyzaishell/commit/18eba0cf02a2c265862e957ba86f1cb24e4bedde)，版本 `0.1.1`。
- [Build and test #38552926592](https://github.com/yanch123/xyzaishell/actions/runs/38552926592)，attempt `1`，2026-10-14 11:16:05 UTC 完成，**9/9 作业 success**。
- Windows Server 2022、Ubuntu 22.04、Ubuntu 24.04 均通过实际安装后的原生 DOM/IPC 验证、新装/重启/同版本重装/卸载重装，以及独立临时数据目录中的真实 **v0.1.0 → v0.1.1 直接升级**。
- 本记录针对上述候选与 CI 产物。这里的安装包不是未来 Release 重建产物；如发布，应另外核验实际 Release 附件。不能用本记录宣称 Windows 10/11 实机或全部桌面交互已验收。

## 2. 作业与测试覆盖

| 作业 | 结果与范围 |
| --- | --- |
| [前端与辅助脚本](https://github.com/yanch123/xyzaishell/actions/runs/38552926592/job/115815378493) | 19 个 Vitest 文件、113 项测试通过；TypeScript/Vite 生产构建通过；69 项 Python pipeline helper 测试通过 |
| [浏览器预览](https://github.com/yanch123/xyzaishell/actions/runs/38552926592/job/115815695245) | 8 项 Playwright 测试通过；此项采用浏览器预览/合成 IPC，不替代原生验证 |
| [Linux Rust 与 DEB](https://github.com/yanch123/xyzaishell/actions/runs/38552926592/job/115815695226) | 核心单测 184 passed；模型/SSH/agent 集成测试分别 15/19/8 passed；另外显式运行真实 OpenSSH、Shell 日志 OpenSSH、两个隔离 Secret Service 测试均通过；fmt、Clippy、打包通过 |
| [Windows Rust 与 NSIS](https://github.com/yanch123/xyzaishell/actions/runs/38552926592/job/115815695273) | 核心单测 199 passed，SSH 生命周期集成测试 17 passed；包括 Windows 私有文件 ACL、凭据存储及 Shell 日志存储/导出测试；fmt、Clippy、打包通过 |
| [Ubuntu 22.04 安装 smoke](https://github.com/yanch123/xyzaishell/actions/runs/38552926592/job/115822098070) | DEB 安装、依赖/启动 smoke 通过 |
| [Ubuntu 24.04 安装 smoke](https://github.com/yanch123/xyzaishell/actions/runs/38552926592/job/115822098176) | 同一 Ubuntu 22.04 构建的 DEB，在 Ubuntu 24.04 安装与启动 smoke 通过 |
| [Ubuntu 22.04 原生与升级](https://github.com/yanch123/xyzaishell/actions/runs/38552926592/job/115822098124) | 全部原生阶段、直接版本升级、最终卸载与清理通过 |
| [Ubuntu 24.04 原生与升级](https://github.com/yanch123/xyzaishell/actions/runs/38552926592/job/115822098149) | 全部原生阶段、直接版本升级、最终卸载与清理通过 |
| [Windows Server 2022 原生与升级](https://github.com/yanch123/xyzaishell/actions/runs/38552926592/job/115822098251) | 全部原生阶段、直接版本升级、最终卸载与清理通过；不等同 Windows 10/11 桌面实测 |

常规 Rust 测试输出中的 ignored 用于必须显式提供隔离环境的测试，不计入普通 passed；Linux 工作流随后显式执行真实 OpenSSH 和 Secret Service 测试，不能把仅编译/ignored 当运行成功。

### 2.1 真实 Shell 日志与导出

Linux 日志明确记录：

- 11:01:48 UTC：`shell_log_real_openssh_records_commands_without_model ... ok`。使用真实 loopback OpenSSH/OS PTY，不配置任何模型供应商。
- 验证 `pwd`、执行 `cd` 前的 cwd、管道、子 shell 命令文本、多行及 heredoc、退出码 0/1、本地接收时间和远端时间字段；`read -s` 后输入的合成密码不变成命令记录。
- 受控 shell `exit` 和执行中断线保留 `unknown`，不伪造成功；关闭 history 后明确产生 gap，不重复冒认上一条命令。
- **真实 OpenSSH 产生的非空记录经服务导出 JSON**，验证命令内容存在且不含合成 `read` 密码。
- Windows/Linux Rust 存储测试另外覆盖非空 CSV/JSON、CSV 公式/换行处理、筛选、不可变命令身份、脱敏、导出私有权限、异常路径/链接拒绝及容量限制。这些是实际存储/服务测试，不能描述为原生 UI 点击非空导出。
- 独立 sshd 审计严格为 **5 次 PTY、5 次 shell、1 次 resize、5 个 session channel，零 exec/subsystem**；包括既有互操作测试的 2 个连接及新日志测试的 3 个连接。daemon、临时账号和临时凭据清理成功。
- 11:01:50 UTC：独立真实 Bash PTY 测试通过，覆盖 POSIX shell 启动生成的 bootstrap、完整文本、cd 前 cwd、状态、`read` 秘密、history gap、非法 UTF-8/控制字符、嵌套 shell，以及退出后返回父 shell 的 cwd。

### 2.2 原生 UI 与数据保留

三个 OS 均独立完成下列两组场景，使用不同的临时 HOME/XDG 或 AppData，不复用档案、数据库或凭据：

1. v0.1.1 新装：通过真实 UI 保存合成 SSH 档案，通过真实 Tauri IPC 与 loopback 合成模型聊天，随后关闭应用并验证进程退出；重启后重新取得本进程历史同意，仅恢复文字。
2. 同版本重装；卸载再安装：各次安装/卸载前后比较 model.toml、connections.toml、history.sqlite3 的字节指纹，并验证档案、精确聊天文本和稳定任务 ID 保留。
3. 独立升级夹具：下载并核对已发布 v0.1.0 的精确包、manifest 和安装后二进制；先在 **旧版应用** 真实建档、聊天和重启。随后直接运行 v0.1.1 安装器，中间不手动卸载、不重建数据目录。升级后再次验证原有指纹、档案、文字和任务 ID。
4. 新版每次应用会话均验证 Shell 日志入口、隐私说明、默认未启用、未连接时双同意框/启动按钮禁用、关闭后重开；点击 JSON/CSV 导出显示 `SHELL_LOG_EMPTY` 对应安全消息，确认没有创建数据库、锁文件、导出目录/文件，也没有触发模型请求。
5. 恢复历史不新增模型请求、不执行脚本/SSH 命令；合成 `<script>…</script>` 内容保持字面文字。真实应用退出检查保留，状态改变操作没有自动重放。
6. 两组场景最后均实际调用卸载器/包管理器卸载，检查应用已移除、最终配置指纹保持，再清理各自临时目录。三平台合计 **6 次最终卸载/夹具清理均成功**。

产物包含 **15 份 passed result.json**（每 OS 5 份阶段结果），对应 **21 次真实已安装应用会话**（每 OS 7 次，包含旧版、新版及重启）。三个 OS 的证据中均没有生成 `read-only-transport-retries.jsonl`，即本轮没有记录到只读传输重试；这不是所有网络环境都不出错的保证。

## 3. 安装包与来源核验

两平台 `artifacts.py verify-package` 均已在精确候选 checkout 下再次通过：绑定源 SHA、版本、平台、包类型、包字节 SHA256 和原始 lockfile 字节哈希。manifest 的工作流均为 `38552926592`、attempt `1`。

| 文件 | 字节数 | SHA256 |
| --- | ---: | --- |
| xyzaishell-0.1.1-linux-x64.deb | 11674162 | `42183191b3df973f857401acf179832e0dd086001b7c0d5df7752432f0f3a4c9` |
| build-linux-x64.json | 1107 | `5d70c09e48c7dce38c2f7e1d465934f168e49c259379ae8ef906173315a02cc9` |
| xyzaishell-0.1.1-windows-x64.exe | 6406840 | `fbea126e304bfae206ad51c768f00a4f91f7df23a71cd43d7f1e60823e0a02b0` |
| build-windows-x64.json | 1132 | `4dc0993f69309dc5070a8543723291eb94cde73aefcee7e10ff107c92913eae2` |

- Cargo.lock：`6fc6cbd3f608a4618eb79711227216647e94135a31faf9e1101d3db253a26c99`。
- package-lock.json：`40ac9586656567419cfe49d17cc51952e3031267a6971bdf09b1619fefa635bb`。
- 实际运行的 Linux 二进制：`c791c94868c68ff280a279170c3ec70a0e352a152d727e638b40f78b330e931b`，两个 Ubuntu 一致；本地只读解包 DEB 后再次核对相同。
- 实际运行的 Windows 二进制：`81b54acbb69f96b156eae085a6a7482f0ebe5260bc6375aafb486ea173a16e75`。

升级基线为 [已发布的 v0.1.0](https://github.com/yanch123/xyzaishell/releases/tag/v0.1.0)，来源 `73c040217d27c002201a8ed593511d198af2dc0e`、发布构建 `38545541374` attempt `1`：

- 旧 DEB：`ee4adf0c58deaf6da45d3598786e3bffb876e600e9148242777c41bba62b98e3`。
- 旧 NSIS：`e3198880b99aa7666558be7ca5a3902ea6087461fa99378678252b21a4f591e3`。
- 旧 Linux 二进制：`0781bb5ac16896e629c9040e1db26e0042c3b9a59f8516d6173ef4536f42094a`。
- 旧 Windows 二进制：`943a29f9b4bea87e9bc61613d47acb23fd76bcbf4fe08b11ccd724177fafda73`。

基线 helper 同时固定包大小、manifest SHA、来源/版本/平台/run；版本升级必须真的替换二进制，不能仅重新安装同版本来满足升级验收。

## 4. 运行环境、通知与截图

- Rust 1.99.0、Node 22.23.3；Linux 包在 Ubuntu 22.04 构建。
- Ubuntu 22.04 原生运行：GTK 3.24.33、WebKitGTK 2.50.4；Ubuntu 24.04：GTK 3.24.41、WebKitGTK 2.52.6，均为该次 runner 实际安装的版本。
- Windows Server 2022 使用进程范围固定的 WebView2/Edge Driver 131.0.2903.86。驱动 Microsoft Authenticode `Valid`，记录下载 archive/二进制 SHA；私有 WebView2 用户数据目录不跨夹具共享。未修改用户机器。
- 两类安装包实际安装后均存在并核对非空的 `third-party/THIRD-PARTY-NOTICES.txt`、`RUST-STANDARD-LIBRARY.html`、`runtime-inventory.json`、`sources/option-ext-0.2.0.crate`；每个 fresh/upgrade 证据目录保存已安装文件 SHA。精确 option-ext 源码归档 SHA 为 `04744f49eae99ab78e0d5c0b603ab218f515ea8cfe5a456d7629ad883a3b6e7d`。
- 已保存 **63 张原生截图**（每 OS 21 张）和浏览器产物中的 **16 个 PNG 文件**（含报告附件副本）；浏览器截图仍仅标记为预览/合成 IPC 证据。
- 视觉抽检 Windows 升级后历史恢复、Ubuntu 22.04 升级后空库导出错误、Ubuntu 24.04 升级后默认禁用日志截图；中文、v0.1.1 标识、字面脚本内容、隐私说明与操作入口可读。另检查了 Windows 升级后的日志弹窗。抽检不能替代完整交互式桌面测试。

## 5. 可获取的 CI 证据

以下是本次 CI artifact 链接，受 GitHub artifact 访问规则与保留期限制；不是正式 Release 的永久附件：

- [Linux 安装包、manifest、SHA256SUMS](https://github.com/yanch123/xyzaishell/actions/runs/38552926592/artifacts/11887942356)
- [Windows 安装包、manifest、SHA256SUMS](https://github.com/yanch123/xyzaishell/actions/runs/38552926592/artifacts/11890310173)
- [Ubuntu 22.04 原生截图与升级证据](https://github.com/yanch123/xyzaishell/actions/runs/38552926592/artifacts/11889975831)
- [Ubuntu 24.04 原生截图与升级证据](https://github.com/yanch123/xyzaishell/actions/runs/38552926592/artifacts/11890390599)
- [Windows Server 原生截图与升级证据](https://github.com/yanch123/xyzaishell/actions/runs/38552926592/artifacts/11888702369)
- [Ubuntu 22.04 安装 smoke](https://github.com/yanch123/xyzaishell/actions/runs/38552926592/artifacts/11889016882)
- [Ubuntu 24.04 安装 smoke](https://github.com/yanch123/xyzaishell/actions/runs/38552926592/artifacts/11890205419)
- [浏览器预览报告](https://github.com/yanch123/xyzaishell/actions/runs/38552926592/artifacts/11886644907)

上述 8 个 artifact ZIP 已全部下载，逐个核对 GitHub API 返回的 SHA256 digest 后解包；9 个作业完整日志及最终 run/jobs/artifact 元数据均已留存。

## 6. 未实测或不在本轮保证内

- Windows 10/11 的真实用户桌面、SmartScreen/未签名安装器提示、完整安装体验；当前 Windows 证据来自 Server 2022 runner。
- 完整 X11/Wayland 桌面、输入法、剪贴板、焦点、高 DPI/多显示器及长期稳定性；Xvfb 原生自动化只覆盖上述流程。
- 用户真实 SSH 服务器和真实模型供应商、真实凭据；模型仅使用 loopback 合成服务，SSH 使用专门临时 OpenSSH 服务。
- **已安装原生 UI 连接 SSH 后点击非空日志导出的端到端流程尚未运行**；非空 JSON 来自真实 OpenSSH + Rust 服务，CSV/JSON 细节来自实际 Rust 存储测试，原生 UI 本轮只验证未启用/空库拒绝。
- v0.1.1 不采集终端输出，不覆盖任意 shell、嵌套 shell 内部命令或被修改 hook 的所有行为，也不是防篡改安全审计。gap/unknown 是明确的不确定状态；需要人工确认终端已处于空闲兼容 shell 提示符后启动，不宣称无人值守全自动。
- 明文日志及导出可能包含未被识别的秘密；脱敏不是绝对保证，应先检查再分享。没有向外部服务上传真实敏感数据。
- 安装包未代码签名；本次并非签名/公证/SmartScreen 声誉验收。
- 未进行实际新版本 Release 发布或验证其附件；如用户批准发布，仍需单独执行受控发布工作流并核对最终 NSIS、DEB、源码 ZIP/TAR.GZ、两个 manifest 与 SHA256SUMS。
