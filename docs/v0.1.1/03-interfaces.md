# 03 — 日志接口

接口类型以 crates/xyz-core/src/shell_log_types.rs 与对应前端 bridge 为实现契约；以下为用户可用能力的边界。

## 记录

id、全局 sequence、SSH session_id/generation、shell_id/sequence、host/port/username、started_at_ms（本机收到）、finished_at_ms、remote_started_at（远端报告）、command、cwd（执行前物理路径）、status（running/completed/unknown）、exit_code、redacted。

同一 Shell 的手动/AI 命令按同一方式采集，不推断输入来源。completed 表示收到了返回提示符事件，不等于返回码为零，也不能证明后台子进程结束。

## Tauri IPC

- get_shell_log_status：状态与错误。
- start_shell_log(session, consent, readinessConfirmed)：显式同意本地明文记录并确认终端就绪后启动集成；启动发送失败不得自动重试。
- set_shell_log_paused(paused)：暂停/恢复现有集成，不重复注入。
- query_shell_log(query)：筛选和 keyset 分页。
- export_shell_log(query, format)：JSON/CSV 筛选快照导出，返回实际路径/条数/字节数。
- shell-log:status：状态通知，带 revision，忽略过时事件。

没有 append、执行、重放、任意文件读取或任意路径写入 IPC。

## 筛选与上限

query 支持 session_id、from_ms、to_ms、contains、before_seq、snapshot_max_seq、limit。默认每页 100，最多 200；关键词按字面查询，不拼 SQL。snapshot_max_seq 在下一页与导出时固定。数据库最多 50,000 条和 64 MiB，达到上限需明确停止保存，不静默清除历史。

## 状态

disabled、starting、recording、paused、gap、disconnected。starting 不代表握手成功；状态只能描述接收到的 Shell 报告，不能保证远端持续完整记录。日志故障不影响左侧手动终端使用。
