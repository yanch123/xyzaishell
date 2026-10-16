# 01 — v0.1.3 发布候选验收

- 候选：`00660aa73474827cbd78e6f821c3e2b9a254dad2`
- 候选完整 CI：[38840120622](https://github.com/yanch123/xyzaishell/actions/runs/38840120622)，attempt 1，11/11 作业通过。
- 发布模式：未签名预发布；不标记 Latest。
- 发布授权：2026-10-16 用户确认发布 v0.1.3，并要求 README 仅为使用说明；该候选包含本次 README 和完整使用指南更新。

## 实际验收

### installation_lifecycle

精确候选在 Windows Server 2022、Ubuntu 22.04/24.04 共 15 个原生阶段通过，覆盖安装、原生 UI/IPC 设置与合成回环模型测试、连接档案、历史、重启、同版本重装及卸载重装。测试使用隔离合成资料，不代表用户桌面或真实服务器验收。

### ssh_model_security

精确候选全部 11 个 CI 作业通过，包括前端测试、14 项浏览器模拟用例、两平台 Rust 测试及严格 Clippy、Ubuntu 22.04/24.04 真实 OpenSSH 与当前 Bash PTY 测试，以及两平台保护存储。已查看浏览器与原生截图；支持范围有界，模型解读不等于独立核验，未知状态不重发。未验证真实外部供应商、安装后 UI 的真实 SSH 或非空日志完整导出流程。

### dependency_notices

Windows NSIS 与 Linux DEB 按锁文件构建，下载的 ZIP 和安装包摘要与 GitHub 元数据、provenance、SHA256SUMS 一致，源码提交、版本、运行 ID 与锁文件摘要匹配。原生阶段检查第三方告知及适用源码资源。未使用代码签名；项目未另行选择开源许可证，不替代所有法律判断。

### version_upgrade

三个托管环境实际从已发布且摘要固定的 v0.1.2 升级到此 v0.1.3 候选。private-v2 连接资料、稳定 ID、模型配置、高级项和历史行保留；恢复模型请求为零，每次新进程重新确认保存。legacy_profile_migration_tested=false：本次不宣称旧 v1 档案或加密凭据迁移验收。

## 独立产物与截图复核

下载并检查 8 个 CI ZIP 的实际大小和 SHA256，匹配 GitHub 元数据。两份安装包匹配各自 checksum/provenance；15 份原生阶段结果全部通过。浏览器日志 14/14，通过截图复核紧凑和宽屏界面；50 张浏览器 PNG、87 张原生 PNG 可解码并完成联系表审阅。详细机器可读结果见同目录 03-native-results-summary.json。

候选安装包（仅本次 CI，发布流程会重新构建，公开文件需另行复核）：

- Linux DEB SHA256：`858255b5afcb98a1d36dea409cd069ee25d42463c85efd6c4f022a9710bae258`
- Windows EXE SHA256：`e8cf55f98421e50840c9438c364920603afc91088af810e82f63de0b714925c6`

## 未完成范围

Windows 10/11 实机、Ubuntu 完整桌面/Wayland、输入法/系统剪贴板、真实外部模型供应商、安装后 UI 的真实 SSH、非空操作日志端到端导出、旧 v1 档案或加密凭据升级尚未完成。托管原生和模拟浏览器证据不替代这些项目。任务摘要不是完整日志，退出码不证明业务成功，脱敏不能覆盖所有未知秘密。

## 发布门槛

激活提交只加入发布意图及此目录普通证据文件。main 工作流对同一精确候选重新执行完整 CI，通过后创建候选标签、源码 ZIP/TAR.GZ、Windows/Linux 安装包、两份 provenance 和校验和。先验证七份文件再公开，公开后再次核对标志、标签和摘要。
