import { TaskContextPanel } from './TaskContextPanel';
import { TaskSecretPanel } from './TaskSecretPanel';
import { useEffect, useId, useRef, useState } from 'react';
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
  const [draftExpanded, setDraftExpanded] = useState(false);
  const draftRegionId = useId();
  const compactComposer = !!task?.owner && !draftExpanded;
  const body = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const connected = !!task && session.state === 'connected' && sameSession(task.session, session.session);
  const bytes = utf8Length(draft.trim());
  const sourceMismatch = !!current?.source && (current.source.host !== session.host || current.source.port !== session.port || current.source.username !== session.username);
  const readOnly = !!current?.restored_read_only || sourceMismatch;
  const canStart = agent.native && agent.ready && !!current && !readOnly && !current.active_run_id && config.ready && session.state === 'connected' && !agent.blocked && !!draft.trim() && bytes <= MAX_MESSAGE_BYTES;
  useEffect(() => { setChecked(false); }, [task?.run_id, task?.phase, task?.round]);
  useEffect(() => { following.current = true; }, [id]);
  useEffect(() => { setDraftExpanded(false); }, [id, task?.run_id]);
  useEffect(() => {
    const scroll = body.current;
    if (!following.current || !scroll) return;
    const card = task?.phase === 'awaiting_display' ? scroll.querySelector('.terminal-task > .terminal-action-card') : null;
    const top = card ? scroll.scrollTop + card.getBoundingClientRect().top - scroll.getBoundingClientRect().top : scroll.scrollHeight;
    scroll.scrollTo({ top });
  }, [id, task?.revision, task?.phase]);
  const submit = () => { if (canStart && current) onStart(current, draft.trim()); };
  const needsInspection = task && (task.phase === 'needs_user' || task.phase === 'needs_secrets' || (task.phase === 'cancelled' && !task.terminal_ready && !task.owner));
  return <>
    <div className="execution-mode-note"><Icon name="terminal" size={15} /><span>真实终端任务 · 与文字聊天分开 · 每次启动单独确认</span></div>
    {readOnly && <div className="notice warning">此聊天为只读历史或来源服务器与当前连接不同，不能从这里启动终端操作。请选择当前服务器的新聊天。</div>}
    {agent.error && <div className="chat-error" role="alert"><span>{agent.error}</span><button className="button secondary" onClick={() => agent.ready ? void agent.refresh() : agent.retryLoad()}>{agent.ready ? '核对任务状态' : '重试加载'}</button></div>}
    {attention && attention.chat_id !== current?.chat_id && <div className="owner-banner" role="status"><span>{attention.owner ? '另一个聊天拥有 AI 写入权。切换页面不会转移或停止任务。' : '另一个聊天的终端状态仍待核实，请先检查并确认就绪。'}</span><button className="button secondary" onClick={() => onSelectChat(attention.chat_id)}>{attention.owner ? '查看正在执行的任务' : '查看待核实的任务'}</button></div>}
    <div className="chat-scroll execution-scroll" ref={body} onScroll={event => { const element = event.currentTarget; following.current = element.scrollHeight - element.scrollTop - element.clientHeight < 64; }}>
      {!task ? <div className="execution-welcome"><div className="ai-symbol"><Icon name="terminal" size={24} /></div><h2>让每一步操作可见</h2><p>输入具体任务，再确认模型接收方与当前终端状态。命令卡片显示后，受支持的普通检查与目录切换可以自动执行。危险命令逐条批准；未知风险会明确标注，无法可靠处理的语法需手动操作。</p><p>先在终端工具栏接入当前空闲 Bash。匹配命令返回与独立 Shell 就绪后，AI 自动分析下一步，不需要每条确认。集成丢失或状态不明时暂停，不推测完成，也不重发未知命令。</p><p>手动终端始终可用，请自行避免与 AI 输入冲突。密码或密钥只在专用连接界面、本机任务秘密文件或终端中填写；不要输入聊天。</p>{!agent.native && <div className="notice">网页预览不能启动真实执行，不会模拟命令结果。</div>}{session.state !== 'connected' && <div className="notice">先连接 SSH，再启动终端任务。</div>}</div> : <section className="terminal-task" aria-label="当前终端任务">
        <div className="task-heading"><span className="task-phase" role="status">{task.phase === 'cancelled' && task.owner ? '停止请求已发出 · 等待发送边界' : TASK_PHASE_LABELS[task.phase]}</span><span>第 {task.round} 轮</span></div>
        <p className="task-instruction">{task.instruction}</p>
        <div className="task-identity" title={`任务 ${task.task_id}\n运行 ${task.run_id}`}>当前任务与运行固定绑定 · {task.target.username}@{task.target.host}:{task.target.port}</div>
        <TaskContextPanel task={task} />
        {task.assistant_text && <article className="local-message assistant-message" aria-label="任务模型分析"><div className="message-meta"><span className="assistant-avatar"><Icon name="spark" size={13} /></span><span>模型分析 · 当前轮</span></div><p>{task.assistant_text}</p></article>}
        {!task.assistant_text && task.phase === 'preparing' && <p className="task-waiting">正在等待模型分析，不代表已执行命令。</p>}
        {task.actions.length > 0 && <details className="previous-actions"><summary>查看之前 {task.actions.length} 条命令的发送记录</summary>{task.actions.map(action => <TerminalActionCard key={action.action_id} task={{ ...task, action, phase: 'completed', owner: false }} agent={agent} connected={connected} displayEnabled={false} />)}</details>}
        {task.action && <TerminalActionCard key={`${task.run_id}:${task.action.action_id}`} task={task} agent={agent} connected={connected} displayEnabled={displayEnabled} />}
        {connected && task.owner && ['needs_user', 'needs_secrets'].includes(task.phase) && <TaskSecretPanel key={`${task.task_id}:${task.run_id}`} task={task} agent={agent} connected={connected} />}
        {task.phase === 'awaiting_completion' && <p className="task-waiting">{task.action?.command_completed ? `前台命令已返回（退出码 ${task.action.exit_code}），仍在等待独立 Shell 就绪证据。` : '命令已发送，正在等待匹配的前台返回与独立 Shell 就绪证据。不会根据静默或提示符外观继续。'}</p>}
        {task.phase === 'unsupported_action' && <div className="notice warning">此动作风险未知且超出当前自动执行支持范围，尚未发送。请停止 AI 后在左侧终端核实并手动处理，再按实际状态开始新任务。</div>}
        {task.error && <div className="inline-error" role="alert">{task.error.safe_message}</div>}
        {!connected && <div className="notice warning">此任务绑定的 SSH 连接已不可用。重连不会恢复旧批准或重发命令。请核实远端状态，并明确开始新任务。</div>}
        {task.phase === 'cancelled' && task.owner && <div className="notice warning">正在等待本地写入操作到达可判定边界。在此之前不会释放 AI 写入权，也不能继续或启动另一个任务。</div>}
        {needsInspection && connected && <div className="task-inspection"><h3>请先检查左侧终端</h3><p>自动执行已暂停。确认已处理交互或运行中的程序，并在工具栏重新建立当前 Shell 集成就绪后，才能继续。此恢复确认不会生成退出码，不代表上一命令成功，也不会重发它。</p><label className="check-field"><input type="checkbox" checked={checked} onChange={event => setChecked(event.target.checked)} disabled={!!pending || !!agent.uncertain[id]} /><span>我已检查终端，并确认当前可接收下一步命令。</span></label><button className="button primary" disabled={!checked || !!pending || !!agent.uncertain[id]} onClick={() => { setChecked(false); void agent.continueTask(task.task_id, task.run_id); }}>{task.phase === 'cancelled' ? '确认终端就绪 · 不重启 AI' : '确认就绪并继续分析'}</button></div>}
        {task.phase === 'completed' && <p className="action-warning">这表示 AI 流程结束，不证明远端任务成功。请以实际终端和独立核实为准。</p>}
        {task.owner && <button className="button secondary stop-task" disabled={pending === 'cancelling' || task.phase === 'cancelled'} onClick={() => void agent.cancel(task.task_id, task.run_id)}>{pending === 'cancelling' || task.phase === 'cancelled' ? '正在停止 AI…' : '停止 AI 任务'}</button>}
        {(task.owner || task.phase === 'cancelled') && <p className="action-warning">停止 AI 只阻止后续步骤，不中断远端程序。需要时使用左侧单独的“发送 Ctrl+C”按钮。</p>}
      </section>}
      {agent.errors[id] && <div className="inline-error" role="alert">{agent.errors[id]}</div>}
      {agent.uncertain[id] && <div className="notice warning" role="alert"><span>本次操作的响应不确定，已阻止重复提交。只读取后端状态核对，不会重发命令。</span><button className="button secondary" onClick={() => void agent.refresh()}>核对任务状态</button></div>}
    </div>
    <div className="task-composer-shell">
      {task?.owner && <button className="task-composer-toggle" aria-expanded={!compactComposer} aria-controls={draftRegionId} onClick={() => setDraftExpanded(value => !value)}>{compactComposer ? '展开新任务草稿' : '收起新任务草稿'}<span>当前任务结束后才能发送</span></button>}
      <div id={draftRegionId} hidden={compactComposer} className="composer-area"><div className="composer"><textarea aria-label="终端任务指令" placeholder="描述要执行的任务… 请勿输入密码或密钥" value={draft} maxLength={MAX_DRAFT_LENGTH} disabled={!current} onChange={event => current && agent.updateDraft(id, event.target.value)} onKeyDown={event => { if (event.key === 'Enter' && (event.ctrlKey || event.metaKey) && !event.nativeEvent.isComposing) { event.preventDefault(); submit(); } }} /><div className="composer-toolbar"><span className="task-model-label" title={config.destination ?? undefined}>{config.model ?? '未配置模型'}</span><button className="save-draft" disabled={!canStart} onClick={submit}>{pending === 'starting' ? '正在启动…' : '核对并开始执行'}<Icon name="arrow" size={15} /></button></div></div><div className={`composer-footnote ${bytes > MAX_MESSAGE_BYTES ? 'over-limit' : ''}`}><span>{bytes > MAX_MESSAGE_BYTES ? '任务指令超过 16 KiB，请缩短' : current?.active_run_id ? '先停止或等待当前聊天生成，再启动终端任务' : agent.blocked ? '已有任务或待核实操作 · 草稿不会发送' : '启动前需要数据授权与当前 Shell 集成就绪'}</span></div></div>
    </div>
  </>;
}
