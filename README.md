# xyzaishell

面向 Windows 和 Linux 的跨平台 AI SSH 终端。左侧保留常规 SSH 终端，右侧提供类似 Cursor 的 AI 对话。AI 根据任务生成命令、通过同一个可见终端执行并分析输出，高风险操作需要用户授权。

## 当前状态

**v0.1 开发中，日期 2026-10-14。** 开发代码按小阶段提交到 `dev`，每阶段分别记录检查结果与未完成项。仅在版本验收通过后合入 `main` 并发布源码及 Windows／Linux 安装包。当前功能与验证状态见[09 开发记录](docs/v0.1/09-development.md)；设计文档不是已实现功能清单。

## 文档导航

本版设计文档统一放在 `docs/v0.1/`，按阅读顺序编号；根目录 README 为入口。

| 文档 | 用途 |
| --- | --- |
| [01 需求与范围](docs/v0.1/01-requirements.md) | 已确认功能、范围排除、建议默认值与待确认事项 |
| [02 系统架构](docs/v0.1/02-architecture.md) | 桌面、SSH、Agent、执行控制与存储的职责 |
| [03 接口设计](docs/v0.1/03-interfaces.md) | 模块契约、数据结构、事件、错误与执行状态 |
| [04 配置规范](docs/v0.1/04-configuration.md) | 模型、SSH 连接、任务三类配置与环境变量约定 |
| [05 安全与风险边界](docs/v0.1/05-security.md) | 授权、凭据、提示注入、断线和明确的使用限制 |
| [06 测试与验收](docs/v0.1/06-testing.md) | 可执行的测试场景、通过条件与验证状态 |
| [07 开发计划与决策](docs/v0.1/07-roadmap.md) | 分阶段实现、待确认选项、延后范围 |
| [08 文档审查记录](docs/v0.1/08-review.md) | 文档检查结果及尚未进行的应用验证 |
| [09 开发记录](docs/v0.1/09-development.md) | 分支流程、阶段进度、验证证据及阻塞 |
| [10 发布准备](docs/v0.1/10-release-readiness.md) | 依赖许可、发布证据与剩余验收 |
| [11 使用指南](docs/v0.1/11-user-guide.md) | 模型配置、SSH 档案、终端任务、秘密与历史 |

示例配置：[模型](config/examples/model.example.toml)、[Windows SSH 连接](config/examples/connections.example.toml)、[Linux SSH 连接](config/examples/connections.linux.example.toml)、[任务](config/examples/task.example.toml)。示例不含可用凭据；具体字段以配置规范为准。

## 产品边界

- 首版客户端目标已确认为 Windows 10／11 x64 与 Ubuntu 22.04／24.04 x64；实现与自动验证进度见开发记录，未覆盖的真实桌面场景仍需实测。其他 Linux 发行版、macOS 和 ARM 不在首批支持承诺内。
- 一个应用实例对应一个 SSH 会话，多台服务器通过多个实例连接。底层 WebView 可能产生辅助进程，不承诺一个窗口严格只有一个 OS 进程。
- 一个实例支持多个 AI 对话，每个对话作为独立任务，同一时间只有一个 AI 任务可以向终端写入。
- 用户手动输入始终可用。用户自行避免与 AI 同时操作，不提供自动人工优先、输入暂停恢复或接管协调。
- 只通过 SSH 终端命令操作；不提供本机 PowerShell／WSL、SFTP 或文件上传下载功能。
- 模型通过用户配置的 OpenAI 兼容接口接入；兼容能力需要逐项测试。
- Jev 等决策模型只预留接口，当前不接入；无法可靠判断时暂停该步，交由用户确认。

## 实现技术基线

Tauri 2、Rust／Tokio、React／TypeScript、xterm.js、russh、Rig，采用共享核心加平台适配层，以 TOML 表达配置。可选 SQLite 历史需要明确启用，保存已知秘密脱敏后的文字。依赖精确版本由 Cargo.lock、package-lock.json 与 rust-toolchain.toml 锁定；阶段验证见开发记录。

Windows 构建 NSIS 安装包，使用 WebView2；Linux 构建 `.deb`，使用系统 WebKitGTK／GTK。当前不提供 AppImage，不能把跨平台理解成任意发行版均可运行。路径、凭据和发布差异见[架构](docs/v0.1/02-architecture.md)与[配置](docs/v0.1/04-configuration.md)。

## 凭据与仓库

真实配置、密码、API Key、私钥、实际终端记录和用户聊天记录不得提交。本仓库接收应用代码、测试、示例和公开设计资料。`.gitignore` 只是降低误提交风险，不是安全隔离措施，提交前仍需检查实际差异。

本项目自身的软件许可证尚未选择；本文档不授予未声明的开源许可。引入依赖时应核对所锁定版本的许可证与分发要求。


## 从源码构建

开发环境需要锁定的 Rust 工具链、Node.js 22.12+、Python 3.10+，以及 [Tauri 平台依赖](https://v2.tauri.app/start/prerequisites/)。安装后的程序不需要 Rust、Node.js 或 Python。

```sh
npm ci
python docs/v0.1/tools/prepare_third_party.py
cargo test --workspace --locked
npm test
npm run tauri -- build
```

第三方告知及适用源码资源会按锁定依赖生成并随包提供。构建需要能够访问官方依赖仓库；已有完整缓存时可用生成器的 `--offline` 选项核验。Windows 签名与其他发布限制见[发布准备](docs/v0.1/10-release-readiness.md)。
