# 01 — v0.2.1 候选验收与预发布授权

日期：2026-10-17（UTC）。本记录用于用户已授权的 v0.2.1 未签名预发布试用版。activation 仅增加发布意图与本目录证据，不改变已测试的应用、依赖、说明或工作流。

## 1. 精确来源与证据核验

- 候选源码：[a1a060c9bc7cff25437ae03d75ec3a37f64b29dd](https://github.com/yanch123/xyzaishell/commit/a1a060c9bc7cff25437ae03d75ec3a37f64b29dd)。
- 候选树：`4724bfa43e803165aa504d67fdfddc018482e4b4`。
- [精确候选 CI 39017449064](https://github.com/yanch123/xyzaishell/actions/runs/39017449064)：2026-10-17 08:38:22 UTC 成功结束，12/12 作业通过。
- 验收人员已下载并校验三份原生 ZIP、逐份检查 15 份生命周期 `result.json`，并复核安装包来源、大小与 SHA-256。本记录依据该次实际检查结果；GitHub API 另已回读确认 12 个作业成功及 artifact 的候选 SHA 绑定。
- 公开升级基线为 v0.1.4；没有发布过 v0.2.0 安装包。本次候选版本为 0.2.1，部分沿用的自动化步骤名称写作 v0.2.0，不作为版本判断依据。
- 用户已授权合并 main 并发布本版 Windows／Linux 安装包和源码。发布模式为 `prerelease`，非 Latest；发布成功仍以最终远端状态与资产核验为准。

## 2. 本版主要变化

上下文展示系统指令、工具定义、固定规则、历史摘要、当前对话五类，当前对话可展开；分类来自真实请求来源，不依赖文本猜测。协议开销单列并计入请求总量，类别内容与协议开销可对账。

LLM 压缩可总结保护范围外的旧用户消息、AI 回复和工具结果，按要求保留目标、限制、纠正、事实、不确定事项和剩余步骤。当前指令、必要完整轮次、系统规则、工具定义和执行权限边界继续保护。Quick 保守保护当前投影中的用户消息与完整摘要，不能靠截断摘要丢弃约束。摘要可能有损，不产生新执行授权；原始归档独立，历史保存默认关闭。保留默认 256000 token、轮后 80%／轮中 95%、默认自动 LLM、同一授权模型、无工具摘要请求、无固定压缩目标比例及原子提交行为；失败、取消和过期结果不替换旧投影。

使用说明已包含在候选的 [README](https://github.com/yanch123/xyzaishell/blob/a1a060c9bc7cff25437ae03d75ec3a37f64b29dd/README.md) 与 [05 使用指南](https://github.com/yanch123/xyzaishell/blob/a1a060c9bc7cff25437ae03d75ec3a37f64b29dd/docs/v0.2.1/05-user-guide.md) 中。

## 3. 精确候选的自动化结果

以下计数分别属于不同平台或独立套件，不跨平台相加，也不把子进程输出重复计入 workspace 总数。

| 检查 | 本次结果 |
| --- | --- |
| 前端 | 34 文件、251 项通过；TypeScript／Vite 生产构建通过 |
| Python 流水线辅助检查 | 99 项通过 |
| 浏览器合成验收 | 26 项通过 |
| Linux workspace 顶层测试 | 519 通过（含 Tauri 1）、5 忽略 |
| Linux 隔离 Secret Service | 独立 2 项通过 |
| Windows workspace 顶层测试 | 441 通过（含 Tauri 1）、1 忽略 |
| Rust 格式及严格 Clippy | Linux／Windows 通过 |
| 真实本地 OpenSSH／PTY fixture | Bash 4.2.46 与发行版 5.1／5.2 作业通过 |
| 安装包构建与 headless smoke | Windows NSIS／Linux DEB 构建、Ubuntu 22.04／24.04 smoke 通过 |
| 原生 UI／生命周期 | 三平台共 15 份 result.json 全部通过 |

## 4. 原生安装、压缩与升级证据

| 实际测试环境 | 原生证据 |
| --- | --- |
| Ubuntu 22.04，Xvfb | [native-acceptance-ubuntu-22.04](https://github.com/yanch123/xyzaishell/actions/runs/39017449064/artifacts/12095204676) |
| Ubuntu 24.04，Xvfb | [native-acceptance-ubuntu-24.04](https://github.com/yanch123/xyzaishell/actions/runs/39017449064/artifacts/12095681201) |
| Windows Server 2022 | [native-acceptance-windows-2022](https://github.com/yanch123/xyzaishell/actions/runs/39017449064/artifacts/12095846542) |

三份 ZIP 已下载校验，15 份生命周期结果通过，覆盖每平台首装、同版本重装、卸载重装、v0.1.4 基线和实际 v0.1.4 → 0.2.1 升级。安装后第三方告知资源齐全。

三平台首装在实际安装应用内通过原生 IPC 验证五类与协议对账、LLM 取消／重试／缩减，以及 LLM → 继续聊天 → Quick → 继续聊天；Quick 后完整摘要保持不变，压缩后的两次继续聊天请求和原始归档均已检查。测试模型为合成 loopback 服务，不代表真实外部模型的摘要质量。

三平台实际从已发布且摘要固定的 v0.1.4 基线升级至此候选，连接、模型设置、历史和稳定 ID 保留；恢复时模型请求为 0。数据均为隔离合成资料，不宣称真实旧用户加密凭据迁移已验收。

## 5. 候选安装包来源与摘要

两包的来源清单均绑定候选 `a1a060c9bc7cff25437ae03d75ec3a37f64b29dd` 与 run `39017449064`，大小及下载后摘要已由验收人员核验。以下为内部安装包文件摘要，**不是 artifact ZIP 摘要，也不是最终 Release 摘要**。

| 候选文件 | 字节数 | SHA-256 |
| --- | ---: | --- |
| [xyzaishell-0.2.1-linux-x64.deb](https://github.com/yanch123/xyzaishell/actions/runs/39017449064/artifacts/12095283555) | 12758996 | `af732a70235fac9764339104e5da573f15f185b5d5c03fe566f6b6dee8e51cce` |
| [xyzaishell-0.2.1-windows-x64.exe](https://github.com/yanch123/xyzaishell/actions/runs/39017449064/artifacts/12095766547) | 7007197 | `8341e738bfdd60366698116a5e7d6067131e382f95823b5453fdf98b60717869` |

Actions artifact 有保留期限，通常需要 GitHub 登录。正式发布会重新构建同一源码；最终下载以该次 Release 的 `SHA256SUMS.txt`、来源清单与实际下载核验为准，不复用本表摘要替代发布核验。

## 6. 保留限制与发布条件

- `windows_10`、`windows_11`、`ubuntu_22_04_desktop`、`ubuntu_24_04_desktop` 四项保持 **pending**。Windows Server 2022 和 Ubuntu Xvfb 自动验收不能替代 Windows 10／11 实机、完整用户桌面、输入法、剪贴板或 Wayland 体验。
- `installation_lifecycle`、`ssh_model_security`、`dependency_notices`、`version_upgrade` 在上述精确候选和实际自动化范围内为 **passed**，不扩大为全部桌面人工验收通过。
- 未使用用户真实 SSH 密码或模型密钥，没有连接生产服务器。真实外部模型、安装后 GUI 连接外部 SSH 的全链流程和长期运行不在本次证明范围内。
- 摘要是可能有损的模型解释；结构与来源检查不能证明语义无遗漏，不能因摘要重新授权或执行命令。无新增 Skills、MCP 或子 Agent 功能。
- 安装包未签名，未配置代码签名或自动更新签名。告知资源核验不替代独立许可证法律判断。
- activation 工作流仍须对同一 source_commit 重新验证并运行全部 CI，再创建指向该候选的全新 tag。公开前后均应回读 Release 状态和七个资产：NSIS、DEB、源码 ZIP／TAR.GZ、两平台 build JSON、SHA256SUMS.txt 的名称、上传状态、大小与摘要。
- 本记录不证明 Release 已发布。最终必须为非 draft、prerelease、非 Latest，四项 pending 不自动转为 passed；不得覆盖旧标签或旧 Release。
