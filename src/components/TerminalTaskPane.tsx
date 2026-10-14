import { useEffect, useRef, useState } from 'react';
import type { ChatSnapshot, ModelConfigStatus } from '../lib/model';
import type { SessionSnapshot } from '../lib/types';
import { sameSession } from '../lib/types';
import type { TerminalAgentController } from '../hooks/useTerminalAgent';
import { MAX_DRAFT_LENGTH, MAX_MESSAGE_BYTES, utf8Length } from '../lib/chats';
import { TASK_PHASE_LABELS } from '../lib/agent';
import { TerminalActionCard } from './TerminalActionCard';
import { Icon } from './Icon';

export function TerminalTaskPane({ agent, current, config, session, onStart, onSelectChat, displayEnabled = true }: {
  agent: TerminalAgentController; current: ChatSnapshot | null; config: ModelConfigStatus; session: SessionSnapshot;
  onStart: (chat: ChatSnapshot, instruction: string) => void; onSelectChat: (chatId: string) => void; displayEnabled?: boolean;
}) {
  const task = current ? agent.tasks[current.task_id] : undefined;
  const attention = agent.owner ?? Object.values(agent.tasks).find(item => item.phase === 'cancelled' && !item.terminal_ready && sameSession(item.session, session.session));
  const id = current?.task_id ?? '';
  const pending = agent.pending[id];
  const draft = agent.drafts[id] ?? '';
  const [checked, setChecked] = useState(false);
  const body = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const connected = !!task && session.state === 'connected' && sameSession(task.session, session.session);
  const bytes = utf8Length(draft.trim());
  const canStart = agent.native && agent.ready && !!current && !current.active_run_id && config.ready && session.state === 'connected' && !agent.blocked && !!draft.trim() && bytes <= MAX_MESSAGE_BYTES;
  useEffect(() => { setChecked(false); }, [task?.run_id, task?.phase, task?.round]);
  useEffect(() => { following.current = true; }, [id]);
  useEffect(() => {
    const scroll = body.current;
    if (!following.current || !scroll) return;
    const card = task?.phase === 'awaiting_display' ? scroll.querySelector('.terminal-task > .terminal-action-card') : null;
    const top = card ? scroll.scrollTop + card.getBoundingClientRect().top - scroll.getBoundingClientRect().top : scroll.scrollHeight;
    scroll.scrollTo({ top });
  }, [id, task?.revision, task?.phase]);
  const submit = () => { if (canStart && current) onStart(current, draft.trim()); };
  const needsInspection = task && (task.phase === 'needs_user' || (task.phase === 'cancelled' && !task.terminal_ready && !task.owner));
  return <>
    <div className="execution-mode-note"><Icon name="terminal" size={15} /><span>真实终端任务 · 与文字聊天分开 · 每次启动单独确认</span></div>
    {agent.error && <div className="chat-error" role="alert"><span>{agent.error}</span><button className="button secondary" onClick={() => agent.ready ? void agent.refresh() : agent.retryLoad()}>{agent.ready ? '核对任务状态' : '重试加载'}</button></div>}
    {attention && attention.chat_id !== current?.chat_id && <div className="owner-banner" role="status"><span>{attention.owner ? '另一个聊天拥有 AI 写入权。切换页面不会转移或停止任务。' : '另一个聊天的终端状态仍待核实，请先检查并确认就绪。'}</span><button className="button secondary" onClick={() => onSelectChat(attention.chat_id)}>{attention.owner ? '查看正在执行的任务' : '查看待核实的任务'}</button></div>}
    <div className="chat-scroll execution-scroll" ref={body} onScroll={event => { const element = event.currentTarget; following.current = element.scrollHeight - element.scrollTop - element.clientHeight < 64; }}>
      {!task ? <div className="execution-welcome"><div className="ai-symbol"><Icon name="terminal" size={24} /></div><h2>让每一步操作可见</h2><p>输入具体任务，再确认模型接收方与当前终端状态。命令卡片显示后，基础策略允许的检查可发送；风险或无法判断的命令会等待你的单次批准。</p><p>普通 SSH 无法可靠判断命令完成。每次写入后，AI 会停下来等你检查终端，再明确继续。不会推测退出码，也不会重发未知命令。</p><p>手动终端始终可用，请自行避免与 AI 输入冲突。密码或密钥只在专用连接界面或终端中手动输入；任务机密配置尚未提供。</p>{!agent.native && <div className="notice">网页预览不能启动真实执行，不会模拟命令结果。</div>}{session.state !== 'connected' && <div className="notice">先连接 SSH，再启动终端任务。</div>}</div> : <section className="terminal-task" aria-label="当前终端任务">
        <div className="task-heading"><span className="task-phase" role="status">{task.phase === 'cancelled' && task.owner ? '停止请求已发出 · 等待发送边界' : TASK_PHASE_LABELS[task.phase]}</span><span>第 {task.round} 轮</span></div>
        <p className="task-instruction">{task.instruction}</p>
        <div className="task-identity" title={`任务 ${task.task_id}\n运行 ${task.run_id}`}>当前任务与运行固定绑定 · {task.target.username}@{task.target.host}:{task.target.port}</div>
        {task.assistant_text && <article className="local-message assistant-message" aria-label="任务模型分析"><div className="message-meta"><span className="assistant-avatar"><Icon name="spark" size={13} /></span><span>模型分析 · 当前轮</span></div><p>{task.assistant_text}</p></article>}
        {!task.assistant_text && task.phase === 'preparing' && <p className="task-waiting">正在等待模型分析，不代表已执行命令。</p>}
        {task.actions.length > 0 && <details className="previous-actions"><summary>查看之前 {task.actions.length} 条命令的发送记录</summary>{task.actions.map(action => <TerminalActionCard key={action.action_id} task={{ ...task, action, phase: 'completed', owner: false }} agent={agent} connected={connected} displayEnabled={false} />)}</details>}
        {task.action && <TerminalActionCard key={`${task.run_id}:${task.action.action_id}`} task={task} agent={agent} connected={connected} displayEnabled={displayEnabled} />}
        {task.error && <div className="inline-error" role="alert">{task.error.safe_message}</div>}
        {!connected && <div className="notice warning">此任务绑定的 SSH 连接已不可用。重连不会恢复旧批准或重发命令。请核实远端状态，并明确开始新任务。</div>}
        {task.phase === 'cancelled' && task.owner && <div className="notice warning">正在等待本地写入操作到达可判定边界。在此之前不会释放 AI 写入权，也不能继续或启动另一个任务。</div>}
        {needsInspection && connected && <div className="task-inspection"><h3>请先检查左侧终端</h3><p>远端结果仍未知。确认已处理交互或运行中的程序，终端回到空的 Shell 提示符后，才能继续。此确认不会生成退出码，也不代表命令成功。</p><label className="check-field"><input type="checkbox" checked={checked} onChange={event => setChecked(event.target.checked)} disabled={!!pending || !!agent.uncertain[id]} /><span>我已检查终端，并确认当前可接收下一步命令。</span></label><button className="button primary" disabled={!checked || !!pending || !!agent.uncertain[id]} onClick={() => { setChecked(false); void agent.continueTask(task.task_id, task.run_id); }}>{task.phase === 'cancelled' ? '确认终端就绪 · 不重启 AI' : '确认就绪并继续分析'}</button></div>}
        {task.phase === 'completed' && <p className="action-warning">这表示 AI 流程结束，不证明远端任务成功。请以实际终端和独立核实为准。</p>}
        {task.owner && <button className="button secondary stop-task" disabled={pending === 'cancelling' || task.phase === 'cancelled'} onClick={() => void agent.cancel(task.task_id, task.run_id)}>{pending === 'cancelling' || task.phase === 'cancelled' ? '正在停止 AI…' : '停止 AI 任务'}</button>}
        {(task.owner || task.phase === 'cancelled') && <p className="action-warning">停止 AI 只阻止后续步骤，不中断远端程序。需要时使用左侧单独的“发送 Ctrl+C”按钮。</p>}
      </section>}
      {agent.errors[id] && <div className="inline-error" role="alert">{agent.errors[id]}</div>}
      {agent.uncertain[id] && <div className="notice warning" role="alert"><span>本次操作的响应不确定，已阻止重复提交。只读取后端状态核对，不会重发命令。</span><button className="button secondary" onClick={() => void agent.refresh()}>核对任务状态</button></div>}
    </div>
    <div className="composer-area"><div className="composer"><textarea aria-label="终端任务指令" placeholder="描述要执行的任务… 请勿输入密码或密钥" value={draft} maxLength={MAX_DRAFT_LENGTH} disabled={!current} onChange={event => current && agent.updateDraft(id, event.target.value)} onKeyDown={event => { if (event.key === 'Enter' && (event.ctrlKey || event.metaKey) && !event.nativeEvent.isComposing) { event.preventDefault(); submit(); } }} /><div className="composer-toolbar"><span className="task-model-label" title={config.destination ?? undefined}>{config.model ?? '未配置模型'}</span><button className="save-draft" disabled={!canStart} onClick={submit}>{pending === 'starting' ? '正在启动…' : '核对并开始执行'}<Icon name="arrow" size={15} /></button></div></div><div className={`composer-footnote ${bytes > MAX_MESSAGE_BYTES ? 'over-limit' : ''}`}><span>{bytes > MAX_MESSAGE_BYTES ? '任务指令超过 16 KiB，请缩短' : current?.active_run_id ? '先停止或等待当前聊天生成，再启动终端任务' : agent.blocked ? '已有任务或待核实操作 · 草稿不会发送' : '启动前需要数据授权与 Shell 就绪确认'}</span></div></div>
  </>;
}
