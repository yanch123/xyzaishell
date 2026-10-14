# 02 — v0.1.0 预发布验收记录

本次按用户发布指令提供预发布测试版，不标记为稳定版或 Latest。

## 精确候选与自动验收

- 候选提交：`73c040217d27c002201a8ed593511d198af2dc0e`
- [完整 CI 38544184281](https://github.com/yanch123/xyzaishell/actions/runs/38544184281)：2026-10-14 UTC 完成，九项作业全部成功。
- 已通过前端测试和生产构建、浏览器预览测试、Windows/Linux Rust 测试及安装包构建、Ubuntu 22.04/24.04 安装启动检查、Windows Server 2022 与两个 Ubuntu 版本的实际安装后原生 UI/IPC 和安装生命周期验收。
- 安装生命周期覆盖关闭重启、同版本重装、卸载重装、文字历史恢复不自动请求，以及最终卸载后的配置保留。
- 该证据不替代 Windows 10/11 实机验收、Ubuntu 完整人工桌面验收或跨版本升级测试。

## 发布资产与版本边界

发布流程将重新构建并验证上述精确候选，先创建草稿，再核实 Windows EXE、Linux DEB、源码 ZIP/TAR.GZ、两个构建来源记录和 SHA256SUMS.txt 共七项附件后公开为预发布版。实际发布结果以 [Release 页面](https://github.com/yanch123/xyzaishell/releases) 和发布工作流为准；本记录不预先声明发布成功。

激活提交仅增加发布意图和本验收记录。应用源码、README 和工作流均保持精确候选内容；标签指向该候选。

## 尚未覆盖的范围

- Windows 10/11 真实用户桌面；Windows Server 自动化结果不能替代。
- Ubuntu Wayland/X11 完整人工流程、输入法、剪贴板及辅助功能。
- 安装后 UI 连接外部 SSH、真实模型供应商与生产凭据、长期稳定性。
- 跨版本升级：首发尚无基线，同版本重装不是跨版本升级。
- 安装包未签名；不得将自动化通过描述为签名或平台认证。

每次 AI 写入命令后，用户仍需核实终端状态再继续。秘密脱敏不保证识别未知或变形秘密。项目自身许可证尚未另行选定，第三方通知不等于全面法律保证。

具体验收状态见仓库 `.github/release-intent.json`；早期自动验收细节见 [01-automated-acceptance.md](01-automated-acceptance.md)。
