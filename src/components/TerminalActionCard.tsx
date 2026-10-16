import { ActionOutput } from './ActionOutput';
import { useEffect, useRef, useState } from 'react';
import { actionIdentity, DELIVERY_LABELS, visibleCommand } from '../lib/agent';
import type { TerminalTaskSnapshot } from '../lib/agent';
import type { TerminalAgentController } from '../hooks/useTerminalAgent';

export function TerminalActionCard({ task, agent, connected, displayEnabled = true }: {
  task: TerminalTaskSnapshot; agent: TerminalAgentController; connected: boolean; displayEnabled?: boolean;
}) {
  const card = useRef<HTMLDivElement>(null);
  const latest = useRef({ task, agent }); latest.current = { task, agent };
  const action = task.action!;
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (task.phase !== 'waiting_approval') return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [task.phase, action.action_id]);
  useEffect(() => {
    if (!displayEnabled || !connected || action.displayed || task.phase !== 'awaiting_display') return;
    let stopped = false;
    let first = 0;
    let second = 0;
    let scheduled = false;
    const rendered = () => {
      const element = card.current;
      if (!element || !element.isConnected || element.closest('[inert]') || document.visibilityState !== 'visible') return false;
      const viewport = element.closest('.chat-scroll')?.getBoundingClientRect();
      const top = Math.max(0, viewport?.top ?? 0);
      const bottom = Math.min(window.innerHeight, viewport?.bottom ?? window.innerHeight);
      return ['.action-target', '.exact-command'].every(selector => {
        const bounds = element.querySelector(selector)?.getBoundingClientRect();
        return !!bounds && bounds.width > 0 && bounds.height > 0 && bounds.bottom > top && bounds.top < bottom;
      });
    };
    const check = () => {
      if (stopped || scheduled || !rendered()) return;
      scheduled = true;
      // Two frames place the exact command and target on screen before native
      // dispatch can begin. Hidden tabs, covered UI and unmounted chats never ACK.
      first = requestAnimationFrame(() => {
        second = requestAnimationFrame(() => {
          scheduled = false;
          if (stopped || !rendered()) return;
          const current = latest.current.task;
          if (current.run_id !== task.run_id || current.action?.action_id !== action.action_id || current.action.display_revision !== action.display_revision) return;
          const identity = actionIdentity(current);
          if (identity) void latest.current.agent.acknowledge(identity);
        });
      });
    };
    const observer = typeof IntersectionObserver === 'undefined' ? null : new IntersectionObserver(check);
    if (card.current) observer?.observe(card.current);
    document.addEventListener('visibilitychange', check);
    window.addEventListener('scroll', check, true);
    window.addEventListener('resize', check);
    check();
    return () => { stopped = true; cancelAnimationFrame(first); cancelAnimationFrame(second); observer?.disconnect(); document.removeEventListener('visibilitychange', check); window.removeEventListener('scroll', check, true); window.removeEventListener('resize', check); };
  }, [task.task_id, task.run_id, task.phase, action.action_id, action.display_revision, action.displayed, connected, displayEnabled]);
  const identity = actionIdentity(task)!;
  const disabled = !connected || !task.owner || !!agent.pending[task.task_id] || !!agent.uncertain[task.task_id] || !action.displayed || now >= action.expires_at_unix_ms;
  const secret = action.kind === 'secret_input';
  const command = visibleCommand(action.command_text);
  return <div className={`terminal-action-card ${action.policy === 'require_approval' || action.policy === 'manual_only' ? 'requires-approval' : ''}`} ref={card} data-action-id={action.action_id} role="group" aria-label={secret ? '精确秘密输入动作' : '精确终端命令'}>
    <div className="action-heading"><span>{action.policy === 'manual_only' ? '未知风险 · 仅手动处理' : action.risk === 'unknown' ? '未知风险 · 需核实后单次批准' : action.policy === 'require_approval' ? secret ? '本次秘密输入需要批准' : '危险操作 · 需要本次批准' : '普通操作 · 就绪后自动执行'}</span><span>{DELIVERY_LABELS[action.delivery]}</span></div>
    <dl className="action-target"><div><dt>服务器</dt><dd dir="ltr">{visibleCommand(task.target.username)}@{visibleCommand(task.target.host)}:{task.target.port}</dd></div></dl>
    <pre className="exact-command" dir="ltr" aria-label={secret ? '秘密输入预览（不含值）' : '准确命令文本'}>{command}</pre>
    {command !== action.command_text && <p className="action-warning">不可见或方向控制字符已明确显示为 Unicode 转义。该显示不改变后端命令。</p>}
    {secret && <p className="action-warning">本次只发送任务字段 {visibleCommand(action.secret_field ?? '未知字段')} 的秘密值，不是 Shell 命令。秘密值不在界面或模型中显示。</p>}
    <p className="action-reason">{action.reason}</p>
    {task.phase === 'waiting_approval' && action.approval_id && <>
      <p className="action-status">只批准本次操作。</p>
      {now >= action.expires_at_unix_ms && <p className="inline-error">本次批准已过期，请停止任务并重新核对。</p>}
      <div className="action-buttons"><button className="button secondary" disabled={disabled} onClick={() => void agent.decide(identity, action.approval_id!, 'deny')}>{secret ? '拒绝本次秘密输入' : '拒绝本次命令'}</button><button className="button danger-button" disabled={disabled} onClick={() => void agent.decide(identity, action.approval_id!, 'approve')}>{secret ? '仅批准本次秘密输入' : '仅批准这条命令'}</button></div>
    </>}
    {action.command_completed && <p className="action-result">命令已返回 · 退出码 {action.exit_code}</p>}
    {!secret && action.delivery !== 'not_sent' && <ActionOutput key={`${task.run_id}:${action.action_id}`} task={task} actionId={action.action_id} agent={agent} connected={connected} />}
    <details className="action-details"><summary>执行详情</summary>
      <p>任务 {task.task_id} · 连接代次 {task.session.generation}</p>
      <p className="action-enter">附加按键：Enter 一次</p>
      <p className="action-warning">作用于此连接当前实际终端。手动更改目录、身份或前台程序可能使判断失效。</p>
      {action.command_completed ? <p>集成报告前台命令已返回，退出码 {action.exit_code}。这不证明业务目标成功或后台工作结束；下一步仍需独立 Shell 就绪证据。</p> : action.delivery !== 'not_sent' && <p>远端执行结果未知。写入收据、安静的输出或提示符都不是可靠退出码。</p>}
    </details>
  </div>;
}
