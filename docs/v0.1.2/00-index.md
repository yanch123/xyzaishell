# 00 — v0.1.2 文档索引

版本：v0.1.2。更新日期：2026-10-14。发布状态和安装包以 GitHub Releases 为准；验证结果以对应精确提交的验收记录为准。

第 01—07 篇保留设计阶段的需求、提案与验收方向，文中的草案状态和拟议接口不能当作最新实现或测试通过证据。第 08 篇说明本版本的实际使用流程；安装包是否可用以 Release 页面为准。

## 1. 本版目标

把 xyzaishell 整理成更符合传统桌面软件习惯的 SSH + AI 工作台：保留左侧终端、右侧 AI，模型在独立设置入口填写，SSH 连接和档案管理统一，普通状态集中到主窗口底部。借鉴 Xshell 一类客户端的会话管理与操作入口思路，不表示已逐项研究、复刻或保证与 Xshell 行为一致。

优先解决以下断点：

1. 用户必须离开软件编辑 TOML 才能配置模型。
2. 临时连接、保存档案、保存凭据分散；保存了无密码档案后，连接时没有补输密码的完整流程。
3. 全局 `session_only` 切换影响全部档案，当前界面无法切回系统安全存储。
4. 普通命令也频繁需要人工确认就绪，且当前日志子 Shell 不保留原交互环境。
5. 多窗口的目标、AI 上下文和日志范围需要明确区分，避免把另一服务器的历史当作当前现场。

界面统一使用“操作日志（终端命令记录）”，菜单简称“操作日志”；本文中的 Shell 日志指同一类命令记录，与底部应用消息不同。当前日志面板仍使用“终端日志”标题；顶层菜单使用“操作日志”。

## 2. 阅读顺序

| 文档 | 回答的问题 |
| --- | --- |
| [01 — 需求与范围](01-requirements.md) | 为什么做、哪些方向已确认、本期不做什么 |
| [02 — 桌面交互设计](02-interaction-design.md) | 主窗口、设置、连接管理和消息面板怎样协作 |
| [03 — 架构与安全边界](03-architecture.md) | 配置、秘密、会话和状态由谁管理 |
| [04 — 接口契约提案](04-interface-contracts.md) | 前后端需要交换什么，哪些状态不可混淆 |
| [05 — 迁移与测试计划](05-migration-and-test-plan.md) | 旧数据如何保留，异常与恢复如何验收 |
| [06 — 分阶段交付计划](06-delivery-plan.md) | 评审、实现和发布各自的准入条件 |
| [07 — 自动执行与当前 Shell 集成](07-auto-execution-shell-integration.md) | 普通命令如何连续执行，临时 hooks 的兼容与安全边界 |
| [08 — 使用指南](08-user-guide.md) | 本版本如何配置、连接、启用命令识别、执行与导出日志 |

第 01—07 篇中的接口名、数据字段和布局细节属于设计提案；“应”“必须”表达设计约束。实际操作以第 08 篇及对应代码为准，测试清单不是通过证据。

## 3. 设计阶段的基线核对与拟议变化

| 设计时 v0.1.1 基线行为 | v0.1.2 拟议变化 |
| --- | --- |
| `SettingsDialog` 展示路径、地址、模型，支持重新加载和测试；不提供三个主要字段的编辑保存 | 独立设置入口编辑 Base URL、API Key、模型名，由程序写入现有配置文件 |
| `ConnectionDialog` 强制确认临时认证，请求固定 `session_only: true` | 新建以“连接”为主操作，通过“保存连接／记住密码”选择持久化；编辑已有档案用保存／取消 |
| `ProfileDialog` 已能保存档案及系统保护的凭据，不是完全没有保存能力 | 与连接入口统一；完善列表操作、编辑和缺失凭据补输 |
| `HostStorage::set_mode` 只允许全局切到 `SessionOnly`；界面说明不能切回 | 使用逐档案保管策略；恢复系统安全保存不依赖全局开关 |
| 无凭据的密码档案可以保存；连接时后端返回 `CREDENTIAL_MISSING` | 先进入专用凭据输入步骤，再显式继续连接 |
| 模型 API Key 在受限本地 TOML 中；SSH 持久凭据按平台保护 | 保留两者不同的存储边界；不声称模型 Key 已迁入系统保险库 |
| 自动风险规则是少量精确字符串白名单，执行后仍需人工就绪 | 重做普通／危险／未知分类，接入当前 Shell 生命周期，支持正常路径自动连续执行 |
| 终端日志在显式启动的干净 Bash 子 Shell 内采集 | 改为当前 Shell 临时 hooks；执行状态与可选日志分别消费，不重写启动文件 |

现状依据：[SettingsDialog](../../src/components/SettingsDialog.tsx)、[ConnectionDialog](../../src/components/ConnectionDialog.tsx)、[ProfileDialog](../../src/components/ProfileDialog.tsx)、[host_storage](../../crates/xyz-core/src/host_storage.rs)、[profiles](../../crates/xyz-core/src/storage/profiles.rs)、[model](../../crates/xyz-core/src/model.rs)、[execution](../../crates/xyz-core/src/execution.rs)、[agent](../../crates/xyz-core/src/agent.rs)。这些链接用于核对基线，不能作为新设计已经实现的证据。

## 4. 与旧文档的关系

- [v0.1 设计](../v0.1/01-requirements.md)与实现中未被本版明确调整的安全边界继续适用；其中历史设计状态不作为当前实现状态的唯一依据。
- [v0.1.1 终端日志](../v0.1.1/01-requirements.md)继续保留。消息面板不替代 Shell 命令日志，不改变日志的默认关闭、显式开启和导出边界。
- 本版扩展到 AI 目标上下文、日志分组、风险分类和当前 Shell 集成。第 07 篇明确替代 v0.1.1 的干净子 Shell 路线及正常路径逐条确认就绪；旧文档只描述旧版本行为。
- 一个进程一个 SSH 连接、一个可见 PTY；多进程各自独立窗口。连接另一目标默认新窗口的具体操作见第 02 篇；不提供 SFTP。
- 文档评审通过、代码完成、自动化通过、原生安装验收和公开发布是不同状态，后续分别记录。

## 5. 开发阶段记录

- [01 — 日志存储](development/01-log-storage-stage.md)
- [02 — 自动执行](development/02-automatic-execution.md)
- [03 — 模型设置与聊天来源](development/03-model-settings-and-chat-source.md)
- [05 — 连接与凭据](development/05-connection-profiles.md)
- [06 — 整合验证](development/06-integrated-stage.md)

- [04 — 当前 Shell 生命周期与独立日志](development/04-current-shell-integration.md)：实现边界、关联注释及真实 PTY / Rust 验证记录。
