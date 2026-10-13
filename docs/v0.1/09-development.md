# 09 开发记录

## 分支与交付

2026-10-13 用户授权开始开发。设计基线提交：`6d7809a31ce802ce008efad1d44fe51113d7dcef`。

- 每个小阶段完成相应检查后提交 `dev`，单独列出未运行项，不将阶段提交等同于最终验收。
- v0.1 功能和验收完成后将 `dev` 合入 `main`，Release 提供源码、Windows 安装包、Linux 安装包及校验和。
- 开发 CI 构建产物与正式 Release 分开。尚未通过验收不创建最终版本声明。
- 不提交真实凭据、终端输出或用户对话。

## 实施进度

| 阶段 | 状态 | 交付与证据 |
| --- | --- | --- |
| M0 设计文档 | 已完成 | 文档审查记录及远程文件核对，不代表运行验证 |
| M1 SSH 与界面 | 进行中 | 桌面骨架、同一 PTY、主机密钥确认、认证、输入输出和尺寸同步 |
| M2 模型与任务 | 待开始 | OpenAI 兼容模型配置、流式对话、任务隔离 |
| M3 自动执行与授权 | 待开始 | 可见命令、单 AI 写入者、高风险授权、取消及未知状态 |
| M4 凭据与恢复 | 待开始 | 三类配置、平台凭据后端、持久化和恢复边界 |
| M5 打包与发布 | 待开始 | Windows/Ubuntu 构建、安装和桌面验收、Release |

## 构建与验收边界

开发环境目前是 Linux 云工作区。Rust 核心与前端检查、两平台 CI 构建和真实安装／桌面验收属于不同证据。必须分别记录；不能用一个替代另一个。应用编译、SSH 联调、模型联调及安装验收结果将在每个小阶段追加。

## M1a 本地开发骨架与 SSH 核心（2026-10-13）

实现：Tauri 桌面宿主、React/xterm 双栏界面、russh 同一 PTY、连接代次隔离、明确主机指纹确认、密码认证、Unix 私钥认证、输入 FIFO 与输出 ACK 背压、尺寸变化与断线。凭据和主机信任此阶段仅在进程内有效；Windows 私钥在 ACL 验证实现前明确拒绝，不降级接受不安全密钥。AI 区域不模拟模型响应。

父级复核已通过：`cargo test -p xyz-core --locked`（13 单元＋17 真实 loopback SSH 集成）、`cargo fmt --all -- --check`、`cargo clippy -p xyz-core --all-targets --locked -- -D warnings`。独立测试服务器仅使用临时合成凭据与密钥，不连接用户服务器。

本地原生桌面编译因缺少 GLib/GObject/GTK/WebKit 开发库而受阻；应用宿主原生编译将由 CI 验证。Linux 云工作区 Chromium 启动受环境限制，未完成真实界面截图与安装验收。构建成功、自动测试通过、真实桌面验收必须分别记录。

### 开发命令

需要 `rust-toolchain.toml` 所列 Rust 工具链、Node.js 22.12+ 与 npm，以及 [Tauri 平台开发依赖](https://v2.tauri.app/start/prerequisites/)。

```sh
npm ci
npm test
npm run build
cargo test -p xyz-core --locked
npm run tauri -- dev
```

完整平台检查使用 `cargo test --workspace --locked` 与 `cargo clippy --workspace --all-targets --locked -- -D warnings`，需要本机平台开发依赖。桌面开发模式会启动 Vite；单独打开 Vite 页面只能审查界面，不提供 SSH IPC，也不会启用浏览器本机命令执行。

M1a 前端复核：7 个测试文件／23 项测试、TypeScript 与 Vite 生产构建通过；生产及全量 npm audit 均为 0 项已知漏洞（检查时结果，不是永久保证）。CI 辅助脚本 13 项测试与 actionlint 通过。独立审查修复了重连后旧 xterm 输出／查询回调可能跨会话的问题，增加回归测试；关闭终端 OSC8 外链跳转。当前小阶段可以提交 `dev` 触发原生 CI，但 M1 的两平台验收尚未结束。
