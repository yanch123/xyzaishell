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
| M1 SSH 与界面 | 基础自动验证通过；交互验收待完成 | 桌面骨架、同一 PTY、主机密钥确认、认证、输入输出和尺寸同步 |
| M2 模型与任务 | 本地检查通过；原生 CI 待执行 | OpenAI 兼容模型配置、流式对话、任务隔离 |
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

### M1a 原生 CI 结果

`d64d7696df262ba3e7e54331ed7d8985b0282af9` 的 [CI 38424935901](https://github.com/yanch123/xyzaishell/actions/runs/38424935901) 全部通过。早期 Tauri Rust／JavaScript API 次版本不一致已修复；两端统一 2.12 系列。

- Windows：12 单元＋15 loopback 测试、Clippy 与 NSIS 安装包生成通过。
- Linux：13 单元＋17 loopback 测试、Clippy 与 DEB 生成通过。
- Ubuntu 22.04／24.04：实际 DEB 校验、安装、依赖链接和 Xvfb 窗口启动通过。
- 上述是候选阶段包；不代表 Windows 10／11 真实交互、输入法、升级／卸载或最终功能验收通过。

## M2 模型配置与流式对话（2026-10-13）

已实现：用户文件提供模型 Base URL／API Key／名称；明确选择 OpenAI Chat Completions（Rig 0.44）；无环境变量 Key 回退；精确目的地授权；运行配置快照；后端多聊天、取消、超时、错误及有界上下文；不读取或发送终端数据、不接受执行工具。此阶段聊天仅保存在内存，任务持久化仍属 M4。

传输层在 Rig 解析前限制请求、网络块、SSE 行／事件和累计长度；禁用自动重试、重定向和隐式代理；仅显式允许本机回环明文 HTTP。错误不转发服务端原文、Key 或请求体。配置文件采用本地安全读取；新增 Windows HANDLE 级所有者／ACL 检查，运行验证交给 Windows CI，不以交叉编译代替。

已复核通过：62 项 Rust 测试（31 单元、14 模型回环 HTTP、17 SSH 回环）、格式、严格 Clippy；45 项前端测试、TypeScript、生产构建、npm audit 0；13 项 CI 工具测试与 actionlint。模型测试使用合成数据和本地模拟服务，不代表任意第三方模型服务的兼容性已验证。

新增待 CI 执行的项目：实际 OpenSSH／操作系统 PTY 测试（默认明确 ignored，专用脚本显式启用）、四项 Playwright 浏览器预览／合成 IPC 测试及截图。云工作区 OpenSSH 认证已成功但 PTY chown 被环境限制阻止；浏览器启动受环境限制，因此不声称这些项目本地通过。浏览器预览／Mock 测试不是原生桌面验收。
