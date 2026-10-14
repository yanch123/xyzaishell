# 04 — 当前 Shell 生命周期与独立日志

日期：2026-10-14。代码目标为 Bash 5.1 / 5.2；本机真实 PTY 为 GNU Bash 5.2.37。双版本 OpenSSH CI 的结果必须另行核实，本文不把未执行的平台记为通过。

## 实现

- 增加 `ShellIntegrationService`，由 SSH 会话管理器持续维护有界实时状态。首次安装需要明确空闲 Bash 确认；连接完成、普通提示符文字、静默和写入成功都不作为就绪。
- 使用当前可见 PTY 安装临时 PS0 / PROMPT_COMMAND hooks，保留 PID、alias、function、PS1、历史配置和原有 hooks；不启动干净子 Shell、不修改 rc 文件、不建立 exec 通道。
- `B`、`E`、`R` 分别表示顶层开始、前台返回和完成原提示符 hooks 后的独立就绪。实例、连接代次、序号和准确文本共同关联；远端仍是不可信的合作式元数据，不产生本机批准。
- 对经过本机风险解析的完整单行命令追加可见、无执行效果的动作注释。准确传输文本在动作预览时绑定，解决默认 `ignoredups` 下重复命令的历史丢失；只有匹配本地动作的日志才去掉该后缀，手动相似注释不被改写。模型观察流移除协议帧，并有界过滤已发出的准确关联后缀回显。
- `ShellLogService.start_shared` 只订阅已就绪集成。日志关闭、暂停、队列满或磁盘失败均不停止执行状态；旧独立启动入口仅保留为内部单元测试夹具。
- 已证明未发送的取消可释放预留并产生新的就绪版本；开始后、不确定发送、错误顺序、缺口和断线不能走此恢复，也不自动重发。
- SSH 快照携带宿主解析的可选不可变 `profile_id`，普通原始连接不能从 IPC 指定该关联，日志保留连接时的关联。

## 支持边界与安全复核修正

支持普通 Ubuntu 提示符、`HISTCONTROL=ignoreboth`、关闭 `lithist`、稳定字符串/数组 PROMPT_COMMAND 和已有 PS0。保留原 EXIT trap。拒绝除 EXIT 外的显式 traps、非空 HISTIGNORE、非默认 histchars、禁用 history/cmdhist/promptvars/interactive_comments，以及不安全提示符展开和不兼容选项；失败保留手动终端。

复核发现并修正两项具体问题：
1. `interactive_comments` 关闭会使动作注释变成命令参数。安装外层在解析含注释的脚本前检查此选项，每次就绪再次检查。
2. WINCH/ALRM 等异步 trap 可以在就绪后读取输入。当前受限契约只接受 EXIT trap，安装和每次就绪都核对完整 trap 快照。

没有宣称能兼容任意 prompt 插件、任意 traps 或仲裁手动/AI 输入。后台任务、嵌套解释器、exec/exit 和历史缺失都保留相应的不确定边界。

## 实际运行结果

- `cargo test -p xyz-core shell_integration -- --nocapture`：11 项通过。覆盖首次确认、独立结束/就绪、过期动作、手动冲突、顺序错误、断线、发送不确定性、未发送取消、超时迟到、日志关闭/暂停/故障、匹配后缀和分片回显过滤。
- `python3 crates/xyz-core/tests/shell_log_pty.py`：12 项真实 PTY 测试通过（另含参数化配置案例），使用 Cargo 生成的真实 bootstrap。覆盖 PID/alias/function、PS0、字符串/数组 hooks、`$?`/非零状态、恢复、管道/pipefail、多行/heredoc、后台前台边界、阻塞 prompt hook、隐藏输入、嵌套 Shell、exit/exec、Ctrl+C、Ubuntu 默认配置、重复动作以及动态不兼容项。
- `cargo test -p xyz-core shell_log -- --nocapture`：41 项通过；真实 OpenSSH 测试明确标记为需独立 fixture，未以忽略计入通过。
- `cargo clippy -p xyz-core --all-targets -- -D warnings`：通过。

`crates/xyz-core/tests/shell_log_openssh.rs` 已改为当前 Shell 安装、等到真实 `R` 后订阅日志。Linux OpenSSH、Bash 5.1、Windows 构建和端到端产品验收由后续集成流水线提供证据。

复核后的夹具修正：Ctrl+C 测试不再在 PS0 的部分 Begin 输出后立即发送信号，而先用 PTY 前台进程组确认 sleep 已接管前台。修正后完整 12 项 PTY 套件连续运行三次，36/36 通过；产品执行逻辑未因此更改。

## 托管 OpenSSH 回归修正

CI run `38581418388` 的 Ubuntu 24.04 OpenSSH 实测暴露了日志消费者的同序号缺口处理错误：`set +o history` 正常产生 `B;1` / `E;1` 后，提示符验证再产生 `G;1;hooks_changed`，旧日志逻辑用 `sequence <= high_sequence` 将这个有效缺口误当重放丢弃。执行消费者已正确进入 Unknown，错误仅使日志状态仍显示 Recording。

改为只丢弃更旧序号；同序号提示符缺口保留已完成的前台结果并将记录状态设为 Gap。补充两个日志单元测试和一个共享解析器回归测试，OpenSSH 用例在继续手动输入前核对两个消费者。修正后本地日志 43/43、生命周期 12/12、严格 Clippy 通过；托管 OpenSSH 重新运行的结果单独记录，不以本地单元测试替代。

同轮修正还保留暂停期间的集成丢失证据：缺口/畸形帧会置位 capture 的永久丢失标志，界面仍可显示 Paused，但恢复返回 `SHELL_LOG_INTEGRATION_LOST`，后续事件不能偷偷重新显示 Recording。用户在 Gap 后再暂停也不能清除该证据；新连接上经核验的 `start_shared` 才重置它。宿主恢复入口额外核对当前生命周期就绪。新增暂停 → 缺口 → 恢复、畸形帧和共享解析器回归覆盖。
