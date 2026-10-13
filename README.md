# xyzaishell

面向 Windows 和 Linux 的跨平台 AI SSH 终端。左侧保留常规 SSH 终端，右侧提供类似 Cursor 的 AI 对话。AI 根据任务生成命令、通过同一个可见终端执行并分析输出，高风险操作需要用户授权。

## 当前状态

**文档阶段，版本 design-v0.1，日期 2026-10-13。** 本仓库目前没有应用实现、可运行演示或 Windows／Linux 安装包。本文档集记录已确认需求、建议技术方案、接口草案、验收标准和后续开发顺序；文档完成不代表接口已冻结、代码已编译或测试已通过。

## 文档导航

本版设计文档统一放在 `docs/v0.1/`，按 01–08 编号；根目录 README 为入口。

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

示例配置：[模型](config/examples/model.example.toml)、[Windows SSH 连接](config/examples/connections.example.toml)、[Linux SSH 连接](config/examples/connections.linux.example.toml)、[任务](config/examples/task.example.toml)。示例不含可用凭据；具体字段以配置规范为准。

## 产品边界

- 首版客户端目标已确认为 Windows 10／11 x64 与 Ubuntu 22.04／24.04 x64；这些是待实现、待实测的验收目标。其他 Linux 发行版、macOS 和 ARM 不在首批支持承诺内。
- 一个应用实例对应一个 SSH 会话，多台服务器通过多个实例连接。底层 WebView 可能产生辅助进程，不承诺一个窗口严格只有一个 OS 进程。
- 一个实例支持多个 AI 对话，每个对话作为独立任务，同一时间只有一个 AI 任务可以向终端写入。
- 用户手动输入始终可用。用户自行避免与 AI 同时操作，不提供自动人工优先、输入暂停恢复或接管协调。
- 只通过 SSH 终端命令操作；不提供本机 PowerShell／WSL、SFTP 或文件上传下载功能。
- 模型通过用户配置的 OpenAI 兼容接口接入；兼容能力需要逐项测试。
- Jev 等决策模型只预留接口，当前不接入；无法可靠判断时暂停该步，交由用户确认。

## 建议技术基线

Tauri 2、Rust／Tokio、React／TypeScript、xterm.js、russh、Rig，采用共享核心加平台适配层，使用本地 SQLite 保存非秘密应用记录，以 TOML 表达配置。该组合是文档建议，依赖版本应在实施阶段做兼容性验证后锁定，不能把本文档当成已验证的依赖清单。

Windows 拟采用 NSIS 安装包和 WebView2；Linux 拟以 `.deb` 为主、AppImage 为可选补充，需验证 WebKitGTK 依赖与桌面环境。打包方式尚未冻结，不能把跨平台理解成任意发行版均可运行。路径、凭据和发布差异见[架构](docs/v0.1/02-architecture.md)与[配置](docs/v0.1/04-configuration.md)。

## 凭据与仓库

真实配置、密码、API Key、私钥、实际终端记录和用户聊天记录不得提交。本仓库只接收示例与公开设计资料。`.gitignore` 只是降低误提交风险，不是安全隔离措施，提交前仍需检查实际差异。

本项目自身的软件许可证尚未选择；本文档不授予未声明的开源许可。引入依赖时应核对所锁定版本的许可证与分发要求。
