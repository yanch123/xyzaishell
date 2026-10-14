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
  const command = visibleCommand(action.command_text);
  return <div className={`terminal-action-card ${action.policy === 'require_approval' ? 'requires-approval' : ''}`} ref={card} data-action-id={action.action_id} role="group" aria-label="精确终端命令">
    <div className="action-heading"><span>{action.policy === 'require_approval' ? '本次命令需要批准' : '基础策略允许 · 先显示再发送'}</span><span>{DELIVERY_LABELS[action.delivery]}</span></div>
    <dl className="action-target"><div><dt>服务器</dt><dd dir="ltr">{visibleCommand(task.target.username)}@{visibleCommand(task.target.host)}:{task.target.port}</dd></div><div><dt>任务</dt><dd>{task.task_id}</dd></div><div><dt>连接代次</dt><dd>{task.session.generation}</dd></div></dl>
    <pre className="exact-command" dir="ltr" aria-label="准确命令文本">{command}</pre>
    {command !== action.command_text && <p className="action-warning">不可见或方向控制字符已明确显示为 Unicode 转义。该显示不改变后端命令。</p>}
    <p className="action-enter">附加按键：Enter 一次</p>
    <p className="action-reason">{action.reason}</p>
    <p className="action-warning">作用于此连接当前实际终端。手动更改目录、身份或前台程序可能使判断失效；不会自动仲裁人工输入。</p>
    {task.phase === 'awaiting_display' && <p className="action-status">等待此命令卡片显示确认，尚未派发。</p>}
    {task.phase === 'waiting_approval' && action.approval_id && <>
      <p className="action-status">批准仅用于这个目标与这条命令一次，不能授权后续命令。</p>
      {now >= action.expires_at_unix_ms && <p className="inline-error">本次批准已过期，请停止任务并重新核对。</p>}
      <div className="action-buttons"><button className="button secondary" disabled={disabled} onClick={() => void agent.decide(identity, action.approval_id!, 'deny')}>拒绝本次命令</button><button className="button danger-button" disabled={disabled} onClick={() => void agent.decide(identity, action.approval_id!, 'approve')}>仅批准这条命令</button></div>
    </>}
    {action.delivery !== 'not_sent' && <p className="action-warning">远端执行结果未知。写入收据、安静的输出或提示符都不是可靠退出码。</p>}
  </div>;
}
