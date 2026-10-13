import { useRef, useState } from 'react';
import { chatTitle, createLocalChat, MAX_CHATS, MAX_DRAFT_LENGTH, MAX_MESSAGES } from '../lib/chats';
import type { LocalChat } from '../lib/chats';
import { Icon } from './Icon';
export function ChatPane({ onSettings, showHistory, onToggleHistory }: { onSettings: () => void; showHistory: boolean; onToggleHistory: () => void }) {
  const [chats, setChats] = useState<LocalChat[]>(() => [createLocalChat()]);
  const [selected, setSelected] = useState<string | null>(null);
  const current = chats.find(chat => chat.id === selected) ?? chats[0];
  const composer = useRef<HTMLTextAreaElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const save = () => {
    const text = current.draft.trim();
    if (!text || current.messages.length >= MAX_MESSAGES) return;
    setChats(items => items.map(chat => chat.id === current.id ? { ...chat, title: chat.messages.length ? chat.title : chatTitle(text), draft: '', messages: [...chat.messages, { id: crypto.randomUUID(), text, createdAt: Date.now() }] } : chat));
    requestAnimationFrame(() => list.current?.scrollTo({ top: list.current.scrollHeight, behavior: 'smooth' }));
  };
  const setDraft = (draft: string) => setChats(items => items.map(chat => chat.id === current.id ? { ...chat, draft: draft.slice(0, MAX_DRAFT_LENGTH) } : chat));
  const newChat = () => {
    if (chats.length >= MAX_CHATS) return;
    const next = createLocalChat(); setChats(items => [...items, next]); setSelected(next.id); composer.current?.focus();
  };
  const choose = (text: string) => { setDraft(text); composer.current?.focus(); };
  return <aside className="chat-pane" aria-label="AI 聊天工作区">
    <div className="pane-toolbar"><div className="chat-title"><Icon name="spark" size={16} /><span>任务聊天</span><span className="subtle-badge">M1</span></div><div className="toolbar-actions"><button className={`icon-button ${showHistory ? 'active' : ''}`} aria-label="切换聊天记录" aria-pressed={showHistory} onClick={onToggleHistory} title="聊天记录"><Icon name="history" size={17} /></button><button className="icon-button" aria-label="新建聊天" onClick={newChat} title={chats.length >= MAX_CHATS ? '已达到 30 个本地聊天上限' : '新建聊天'} disabled={chats.length >= MAX_CHATS}><Icon name="plus" size={18} /></button></div></div>
    <div className="chat-subheader"><span className="current-chat-title">{current.title}</span><span className="local-badge">仅本次窗口</span></div>
    {showHistory && <nav className="chat-history" aria-label="聊天列表"><div className="history-heading">当前窗口的聊天<span>{chats.length} / {MAX_CHATS}</span></div>{chats.map((chat, index) => <button key={chat.id} className={chat.id === current.id ? 'selected' : ''} onClick={() => setSelected(chat.id)} aria-current={chat.id === current.id ? 'page' : undefined}><Icon name="message" size={15} /><span>{chat.title === '新任务' ? `新任务 ${index + 1}` : chat.title}</span>{chat.draft && <span className="draft-dot" title="有未保存草稿" />}</button>)}</nav>}
    <div className="chat-scroll" ref={list}>
      {!current.messages.length ? <div className="chat-welcome"><div className="ai-symbol"><Icon name="spark" size={24} /></div><h2>终端旁的 AI 工作空间</h2><p>将一个目标交给一个聊天。<br />在这里整理思路，准备下一项远程任务。</p><div className="availability-card"><span className="availability-title"><span className="status-dot waiting" />AI 尚未接入</span><p>当前 M1 版本提供真实 SSH 终端。聊天仅保存本次窗口内的草稿，不调用模型，不执行命令。</p><button onClick={onSettings}>查看开发阶段<Icon name="chevron" size={14} /></button></div><div className="suggestion-label">先记下你想做的事</div><button className="suggestion" onClick={() => choose('了解这台服务器的系统、资源和运行状态。')}><Icon name="server" size={16} /><span>了解服务器运行状态</span><Icon name="plus" size={14} /></button><button className="suggestion" onClick={() => choose('排查服务异常，先观察日志和状态，再确认修改。')}><Icon name="terminal" size={16} /><span>整理一次故障排查任务</span><Icon name="plus" size={14} /></button></div> : <div className="messages">{current.messages.map(message => <article className="local-message" key={message.id}><div className="message-meta"><span className="user-avatar">你</span><span>任务草稿</span><span className="message-tag">未发送</span></div><p>{message.text}</p></article>)}<div className="chat-system-note"><Icon name="info" size={15} /><span>以上内容仅保存在此窗口内存中。AI 尚未接入，没有模型回复或终端执行。</span></div></div>}
    </div>
    <div className="composer-area"><div className="composer"><textarea ref={composer} aria-label="任务草稿" placeholder="写下任务想法… 请勿粘贴密码或密钥" value={current.draft} maxLength={MAX_DRAFT_LENGTH} onChange={event => setDraft(event.target.value)} onKeyDown={event => { if (event.key === 'Enter' && (event.ctrlKey || event.metaKey) && !event.nativeEvent.isComposing) { event.preventDefault(); save(); } }} /><div className="composer-toolbar"><button className="model-selector" onClick={onSettings}><span className="status-dot" />未配置模型<Icon name="chevron" size={12} /></button><button className="save-draft" onClick={save} disabled={!current.draft.trim() || current.messages.length >= MAX_MESSAGES} title="保存到本地聊天 · Ctrl / ⌘ + Enter">保存草稿<Icon name="arrow" size={15} /></button></div></div><div className="composer-footnote"><span>{current.messages.length >= MAX_MESSAGES ? '此聊天已满，请新建聊天' : '草稿不发送给模型 · 关闭窗口后清除'}</span>{current.draft.length > MAX_DRAFT_LENGTH * .8 && <span>{current.draft.length}/{MAX_DRAFT_LENGTH}</span>}</div></div>
    <div className="chat-status"><Icon name="lock" size={12} /><span>AI 写入者：无</span><span className="status-right">无待审核操作</span></div>
  </aside>;
}
