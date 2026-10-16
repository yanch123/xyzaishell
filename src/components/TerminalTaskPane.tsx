import { TaskContextPanel } from './TaskContextPanel';
import { TaskSecretPanel } from './TaskSecretPanel';
import type { ChatSnapshot, ModelConfigStatus } from '../lib/model';
import type { SessionSnapshot } from '../lib/types';
import { sameSession } from '../lib/types';
import type { TerminalAgentController } from '../hooks/useTerminalAgent';
import { TASK_PHASE_LABELS } from '../lib/agent';
import { TerminalActionCard } from './TerminalActionCard';
import { Icon } from './Icon';

export function TerminalTaskPane({ agent, current, session, onSelectChat, displayEnabled = true }: {
  agent: TerminalAgentController; current: ChatSnapshot | null; config: ModelConfigStatus; session: SessionSnapshot;
  onStart?: (chat: ChatSnapshot, instruction: string) => void; onSelectChat: (chatId: string) => void; displayEnabled?: boolean;
}) {
  const task = current ? agent.tasks[current.task_id] : undefined;
  const attention = agent.owner ?? Object.values(agent.tasks).find(item => item.phase === 'cancelled' && !item.terminal_ready && sameSession(item.session, session.session));
  const id = current?.task_id ?? '';
  const pending = agent.pending[id];
  const connected = !!task && session.state === 'connected' && sameSession(task.session, session.session);
  const conversational = !!task && !task.action && !task.actions.length && (task.phase === 'preparing' || task.phase === 'completed');
  const transcriptUser = !!task && current?.messages.some(message => message.run_id === task.run_id && message.role === 'user');
  const transcriptAssistant = !!task && current?.messages.some(message => message.run_id === task.run_id && message.role === 'assistant' && !!message.text);
  const needsInspection = task && (task.phase === 'needs_user' || task.phase === 'needs_secrets' || (task.phase === 'cancelled' && !task.terminal_ready && !task.owner));
  return <>
    {agent.error && <div className="chat-error" role="alert"><span>{agent.error}</span><button className="button secondary" onClick={() => agent.ready ? void agent.refresh() : agent.retryLoad()}>{agent.ready ? '核对任务状态' : '重试加载'}</button></div>}
    {attention && attention.chat_id !== current?.chat_id && <div className="owner-banner" role="status"><span>{attention.owner ? '另一个聊天正在操作终端。' : '另一个任务需要核对终端状态。'}</span><button className="button secondary" onClick={() => onSelectChat(attention.chat_id)}>{attention.owner ? '查看正在执行的任务' : '查看待核实的任务'}</button></div>}
    <div className="conversation-task-timeline">
      {task && !(conversational && task.phase === 'completed' && transcriptAssistant) && <section className="terminal-task" aria-label="当前终端任务">
        {!conversational && task.phase !== 'budget_paused' && <div className="task-heading"><span className="task-phase" role="status">{task.phase === 'cancelled' && task.owner ? '停止请求已发出 · 等待发送边界' : TASK_PHASE_LABELS[task.phase]}</span><span>第 {task.round} 轮</span></div>}
        {!transcriptUser && <p className="task-instruction">{task.instruction}</p>}
        {!conversational && <TaskContextPanel task={task} />}
        {task.assistant_text && !transcriptAssistant && <article className="local-message assistant-message" aria-label="任务模型分析"><div className="message-meta"><span className="assistant-avatar"><Icon name="spark" size={13} /></span><span>模型分析 · 当前轮</span></div><p>{task.assistant_text}</p></article>}
        {!task.assistant_text && task.phase === 'preparing' && <p className="task-waiting">正在思考…</p>}
        {task.actions.length > 0 && <details className="previous-actions"><summary>查看之前 {task.actions.length} 条命令的发送记录</summary>{task.actions.map(action => <TerminalActionCard key={action.action_id} task={{ ...task, action, phase: 'completed', owner: false }} agent={agent} connected={connected} displayEnabled={false} />)}</details>}
        {task.action && <TerminalActionCard key={`${task.run_id}:${task.action.action_id}`} task={task} agent={agent} connected={connected} displayEnabled={displayEnabled} />}
        {connected && task.owner && ['needs_user', 'needs_secrets'].includes(task.phase) && <TaskSecretPanel key={`${task.task_id}:${task.run_id}`} task={task} agent={agent} connected={connected} />}
        {task.phase === 'awaiting_completion' && <p className="task-waiting">{task.action?.command_completed ? `前台命令已返回（退出码 ${task.action.exit_code}），仍在等待独立 Shell 就绪证据。` : '正在等待命令返回…'}</p>}
        {task.phase === 'unsupported_action' && <div className="notice warning">此操作暂不支持自动执行，尚未发送。请在左侧终端核实。</div>}
        {task.error && <div className="inline-error" role="alert">{task.error.safe_message}</div>}
        {!connected && <div className="notice warning">此任务绑定的 SSH 连接已不可用。重连不会恢复旧批准或重发命令。请核实远端状态，并明确开始新任务。</div>}
        {task.phase === 'cancelled' && task.owner && <div className="notice warning">正在等待本地写入操作到达可判定边界。在此之前不会释放 AI 写入权，也不能继续或启动另一个任务。</div>}
        {task.phase === 'budget_paused' && <div className="task-budget-pause"><p>已处理一轮，需要继续吗？</p><button className="button primary" disabled={!connected || !!pending || !!agent.uncertain[id] || task.error?.code === 'TASK_TOTAL_BUDGET_EXHAUSTED'} onClick={() => void agent.continueTask(task.task_id, task.run_id)}>继续任务</button></div>}
        {needsInspection && connected && <div className="task-inspection"><h3>请先检查左侧终端</h3><p>处理左侧终端中的交互后，点击继续。状态未核实前不会执行或重发命令。</p><button className="button primary" disabled={!!pending || !!agent.uncertain[id]} onClick={() => void agent.continueTask(task.task_id, task.run_id)}>核对状态并继续</button></div>}
        {task.phase === 'completed' && !conversational && <p className="action-warning">任务流程已结束，请以实际结果为准。</p>}
        {task.owner && <button className="button secondary stop-task" disabled={pending === 'cancelling' || task.phase === 'cancelled'} onClick={() => void agent.cancel(task.task_id, task.run_id)}>{pending === 'cancelling' || task.phase === 'cancelled' ? '正在停止 AI…' : '停止 AI 任务'}</button>}
        {(task.owner || task.phase === 'cancelled') && <p className="action-warning">停止 AI 不会终止远端程序。</p>}
      </section>}
      {agent.errors[id] && <div className="inline-error" role="alert">{agent.errors[id]}</div>}
      {agent.uncertain[id] && <div className="notice warning" role="alert"><span>本次操作的响应不确定，已阻止重复提交。只读取后端状态核对，不会重发命令。</span><button className="button secondary" onClick={() => void agent.refresh()}>核对任务状态</button></div>}
    </div>
  </>;
}
