import type { HistoryStatus } from '../lib/storageBridge';
import { useEffect, useRef, useState } from 'react';
import { MAX_CHATS, MAX_DRAFT_LENGTH, MAX_MESSAGE_BYTES, MAX_MESSAGES, MAX_TRANSCRIPT_BYTES, MAX_ASSISTANT_BYTES, utf8Length } from '../lib/chats';
import { MAX_CONCURRENT_CHATS } from '../lib/model';
import type { ModelChatController } from '../hooks/useModelChat';
import { Icon } from './Icon';
import type { TerminalAgentController } from '../hooks/useTerminalAgent';
import type { SessionSnapshot } from '../lib/types';
import type { ChatSnapshot } from '../lib/model';
import { TerminalTaskPane } from './TerminalTaskPane';

const MESSAGE_LABELS = { complete: '已完成', streaming: '正在生成', cancelled: '已停止 · 回复不完整', error: '出错 · 回复不完整' };
export function ChatPane({ model, onSettings, showHistory, onToggleHistory, onConsent, agent, session, onStartTask, displayEnabled = true, historyStatus }: {
  historyStatus?: HistoryStatus | null;
  agent?: TerminalAgentController; session?: SessionSnapshot; onStartTask?: (chat: ChatSnapshot, instruction: string) => void; displayEnabled?: boolean;
  model: ModelChatController; onSettings: () => void; showHistory: boolean; onToggleHistory: () => void; onConsent: (chatId: string) => void;
}) {
  const { chats, current, config } = model;
  const [mode, setMode] = useState<'chat' | 'execute'>('chat');
  const composer = useRef<HTMLTextAreaElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const stickToBottom = useRef(true);
  const draft = current ? model.drafts[current.chat_id] ?? '' : '';
  const draftBytes = utf8Length(draft.trim());
  const pending = current ? model.pending[current.chat_id] : undefined;
  const busy = !!current?.active_run_id || !!pending;
  const terminalBusy = !!current && !!agent?.tasks[current.task_id]?.owner;
  const capacityFull = model.runningCount >= MAX_CONCURRENT_CHATS;
  const transcriptBytes = current?.messages.reduce((sum, message) => sum + utf8Length(message.text), 0) ?? 0;
  const full = !!current && (current.messages.length > MAX_MESSAGES - 2 || transcriptBytes + draftBytes + MAX_ASSISTANT_BYTES > MAX_TRANSCRIPT_BYTES);
  const differentTarget = !!current?.source && !!session && (session.state !== 'connected' || current.source.host !== session.host || current.source.port !== session.port || current.source.username !== session.username);
  const readOnly = !!current?.restored_read_only || differentTarget;
  const canSend = !readOnly && model.native && model.ready && !!current && config.ready && !busy && !terminalBusy && !capacityFull && !full && !!draft.trim() && draftBytes <= MAX_MESSAGE_BYTES;
  const updateDraft = (value: string) => { if (current) model.updateDraft(current.chat_id, value); };
  const send = () => {
    if (!canSend || !current) return;
    if (!config.consented) { onConsent(current.chat_id); return; }
    void model.send(current.chat_id);
  };
  const choose = (text: string) => { updateDraft(text); composer.current?.focus(); };
  useEffect(() => { stickToBottom.current = true; }, [current?.chat_id]);
  useEffect(() => {
    if (stickToBottom.current) list.current?.scrollTo({ top: list.current.scrollHeight });
  }, [current?.chat_id, current?.revision]);
  return <aside className={`chat-pane ${mode === 'execute' ? 'execution-mode' : ''}`} aria-label="AI 聊天工作区">
    <div className="pane-toolbar"><div className="chat-title"><Icon name="spark" size={16} /><span>任务聊天</span><span className="subtle-badge">v0.1.2</span></div><div className="toolbar-actions"><button className={`icon-button ${showHistory ? 'active' : ''}`} aria-label="切换聊天记录" aria-pressed={showHistory} onClick={onToggleHistory} title="聊天记录"><Icon name="history" size={17} /></button><button className="icon-button" aria-label="新建聊天" onClick={() => void model.createChat()} title={chats.length >= MAX_CHATS ? `已达到 ${MAX_CHATS} 个聊天上限` : '新建聊天'} disabled={!model.ready || model.creating || chats.length >= MAX_CHATS}><Icon name="plus" size={18} /></button></div></div>
    <div className="chat-target" tabIndex={0} aria-label="当前 AI 执行目标">当前服务器：{session?.state === 'connected' ? `${session.username}@${session.host}:${session.port ?? 22}` : '未连接'}{session?.state === 'connected' && <span> · 会话 {session.session?.generation}</span>}</div>
    {current && <div className="chat-source" tabIndex={0} aria-label="聊天来源">{current.source ? `来源：${current.source.username}@${current.source.host}:${current.source.port} · 会话 ${current.source.session_id} / ${current.source.generation}` : '来源：未绑定服务器（旧历史不会推断关联）'}{current.restored_read_only ? ' · 已恢复历史，仅供阅读' : differentTarget ? ' · 非当前目标，仅供阅读；请新建聊天' : ''}</div>}
    <div className="chat-subheader"><span className="current-chat-title">{current?.title ?? '正在加载聊天…'}</span><span className="local-badge">{historyStatus?.enabled ? '明文历史已启用' : '默认内存 · 历史需启用'}</span></div>
    {agent && <div className="chat-mode-switch" role="tablist" aria-label="聊天或终端执行"><button role="tab" aria-selected={mode === 'chat'} className={mode === 'chat' ? 'selected' : ''} onClick={() => setMode('chat')}>文字聊天</button><button role="tab" aria-selected={mode === 'execute'} className={mode === 'execute' ? 'selected' : ''} onClick={() => setMode('execute')}>执行终端任务{agent.owner && <span className="mode-running-dot" />}</button></div>}
    {showHistory && <nav className="chat-history" aria-label="聊天列表"><div className="history-heading">当前工作区聊天<span>{chats.length} / {MAX_CHATS}</span></div>{chats.map((chat, index) => <button key={chat.chat_id} className={chat.chat_id === current?.chat_id ? 'selected' : ''} onClick={() => model.selectChat(chat.chat_id)} aria-current={chat.chat_id === current?.chat_id ? 'page' : undefined}><Icon name="message" size={15} /><span>{chat.title === '新任务' ? `新任务 ${index + 1}` : chat.title}</span>{agent?.tasks[chat.task_id]?.owner ? <span className="history-running">执行中</span> : chat.active_run_id ? <span className="history-running">生成中</span> : model.drafts[chat.chat_id] ? <span className="draft-dot" title="有未发送草稿" /> : null}</button>)}</nav>}
    {model.error && <div className="chat-error" role="alert"><Icon name="info" size={15} /><span>{model.error}</span>{!model.ready && <button className="button secondary" onClick={model.retryLoad}>重试加载</button>}</div>}
    {mode === 'execute' && agent && session && onStartTask ? <TerminalTaskPane agent={agent} current={current} config={config} session={session} onStart={onStartTask} onSelectChat={model.selectChat} displayEnabled={displayEnabled} /> : <>
    <div className="chat-scroll" ref={list} onScroll={event => { const element = event.currentTarget; stickToBottom.current = element.scrollHeight - element.scrollTop - element.clientHeight < 64; }}>
      {!current?.messages.length ? <div className="chat-welcome"><div className="ai-symbol"><Icon name="spark" size={24} /></div><h2>聊清思路，再动终端</h2><p>在终端旁讨论、解释和规划。<br />每个聊天保留独立的任务与上下文。</p><div className="availability-card"><span className="availability-title"><span className={`status-dot ${config.ready ? 'online' : 'waiting'}`} />{!model.native ? '网页预览 · 模型连接不可用' : config.ready ? config.consented ? '模型已配置 · 仅聊天' : '模型已配置 · 待确认接收方' : '先配置你的模型'}</span><p>模型只接收你主动发送的聊天，不读取终端、不执行命令。请勿在聊天中输入密码或密钥。</p><button onClick={onSettings}>{config.ready ? '查看模型与发送范围' : '查看本机配置指南'}<Icon name="chevron" size={14} /></button></div><div className="suggestion-label">开始一段讨论</div><button className="suggestion" disabled={!current} onClick={() => choose('帮我制定检查 Linux 服务器运行状态的步骤，解释每一步的目的。')}><Icon name="server" size={16} /><span>讨论服务器检查步骤</span><Icon name="plus" size={14} /></button><button className="suggestion" disabled={!current} onClick={() => choose('如何安全地排查服务异常？先给出只读检查思路。')}><Icon name="terminal" size={16} /><span>整理故障排查思路</span><Icon name="plus" size={14} /></button></div> : <div className="messages" role="log" aria-label="当前聊天消息" aria-live="off">{current.messages.map(message => <article className={`local-message ${message.role === 'assistant' ? 'assistant-message' : ''}`} key={message.id} aria-label={message.role === 'assistant' ? '模型回复' : '你的消息'}><div className="message-meta"><span className={message.role === 'assistant' ? 'assistant-avatar' : 'user-avatar'}>{message.role === 'assistant' ? <Icon name="spark" size={13} /> : '你'}</span><span title={message.destination ?? undefined}>{message.role === 'assistant' ? message.model ?? '模型' : '你'}</span><span className={`message-tag ${message.state}`}>{message.role === 'user' ? '已提交' : MESSAGE_LABELS[message.state]}</span></div>{message.text ? <p>{message.text}</p> : <p className="empty-response">{message.state === 'streaming' ? '正在等待模型响应…' : '没有收到回复正文。'}</p>}{message.error && <div className="inline-error" role="alert">{message.error.safe_message}</div>}</article>)}<div className="chat-system-note"><Icon name="info" size={15} /><span>文字回复本身不会执行命令；历史摘要不代表远端成功，也不会重放操作。请独立核实实际终端状态。</span></div></div>}
    </div>
    <div className="composer-area">{current && model.errors[current.chat_id] && <div className="inline-error" role="alert">{model.errors[current.chat_id]}</div>}<div className="composer"><textarea ref={composer} aria-label="聊天消息" placeholder="输入消息… 请勿粘贴密码或密钥" value={draft} disabled={!current || readOnly} maxLength={MAX_DRAFT_LENGTH} onChange={event => updateDraft(event.target.value)} onKeyDown={event => { if (event.key === 'Enter' && (event.ctrlKey || event.metaKey) && !event.nativeEvent.isComposing) { event.preventDefault(); send(); } }} /><div className="composer-toolbar"><button className="model-selector" onClick={onSettings} title={config.destination ?? '配置模型'}><span className={`status-dot ${config.ready ? 'online' : ''}`} /><span>{config.model ?? '未配置模型'}</span><Icon name="chevron" size={12} /></button>{busy ? <button className="save-draft stop-generation" disabled={!!pending || !current?.active_run_id} onClick={() => current && void model.cancel(current.chat_id)}>{pending === 'cancelling' ? '正在停止…' : pending === 'sending' ? '正在提交…' : '停止生成'}<Icon name="close" size={14} /></button> : <button className="save-draft" onClick={send} disabled={!canSend} title="发送聊天 · Ctrl / ⌘ + Enter">{config.consented ? '发送' : '确认并发送'}<Icon name="arrow" size={15} /></button>}</div></div><div className={`composer-footnote ${draftBytes > MAX_MESSAGE_BYTES ? 'over-limit' : ''}`}><span>{!model.native ? '网页预览不发送 · 草稿仅存于当前窗口' : readOnly ? '此聊天仅供阅读；新建聊天会使用当前目标，不复用历史批准' : full ? '此聊天已达容量限制，请新建聊天' : draftBytes > MAX_MESSAGE_BYTES ? '消息超过 16 KiB，请缩短后发送' : terminalBusy ? '此聊天的终端任务尚未结束，可切换执行页查看或停止' : busy ? '切换聊天不会停止当前生成' : capacityFull ? '已有 4 个聊天生成中，请等待或停止一个' : '仅发送当前聊天 · Ctrl / ⌘ + Enter'}</span>{draftBytes > MAX_MESSAGE_BYTES * .8 && <span>{draftBytes} / {MAX_MESSAGE_BYTES} B</span>}</div></div>
    </>}
    <div className="chat-status"><Icon name="lock" size={12} /><span title={current ? `任务 ID: ${current.task_id}` : undefined}>{agent ? agent.owner ? 'AI 写入权属于固定任务' : 'AI 终端执行：待明确启动' : 'AI 终端执行：未启用'}</span><span className="status-right" role="status">{model.runningCount ? `${model.runningCount} 个聊天生成中` : '无生成中的聊天'}</span></div>
  </aside>;
}
