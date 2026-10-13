import { useRef, useState } from 'react';
import type { CSSProperties } from 'react';
import { useModelChat } from './hooks/useModelChat';
import type { ModelConfigStatus } from './lib/model';
import { ModelConsentDialog } from './components/ModelConsentDialog';
import { useSshSession } from './hooks/useSshSession';
import { TerminalPane } from './components/TerminalPane';
import { ChatPane } from './components/ChatPane';
import { ConnectionDialog } from './components/ConnectionDialog';
import { HostKeyDialog } from './components/HostKeyDialog';
import { SettingsDialog } from './components/SettingsDialog';
import { Icon } from './components/Icon';
import { isActiveSession, SESSION_LABELS } from './lib/types';

export default function App() {
  const ssh = useSshSession();
  const model = useModelChat();
  const [consent, setConsent] = useState<{ config: ModelConfigStatus; chatId?: string } | null>(null);
  const requestConsent = (chatId?: string) => { setModal(null); setConsent({ config: model.config, chatId }); };
  const confirmConsent = async () => {
    if (!consent || !await model.confirmDestination(consent.config)) return;
    const chatId = consent.chatId;
    setConsent(null);
    if (chatId) await model.send(chatId);
  };
  const [modal, setModal] = useState<'connect' | 'settings' | null>(null);
  const [history, setHistory] = useState(false);
  const [split, setSplit] = useState(64);
  const workspace = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);
  const available = ssh.native && ssh.ready;
  const active = isActiveSession(ssh.snapshot.state);
  const hasModal = !!modal || !!ssh.challenge || !!consent;
  return <>
    <div className="app-shell" inert={hasModal}>
      <header className="app-header"><div className="brand"><div className="brand-mark"><Icon name="terminal" size={19} /></div><span>xyzai<span className="brand-light">shell</span></span><span className="version-tag">PREVIEW</span></div><div className="header-workspace"><span className={`status-dot ${ssh.snapshot.state === 'connected' ? 'online' : ''}`} /><span>{ssh.snapshot.host || '未连接服务器'}</span><span className="workspace-divider">/</span><span>SSH 工作空间</span></div><button className="header-settings icon-button" aria-label="打开设置" title="工作空间设置" onClick={() => setModal('settings')}><Icon name="settings" size={19} /></button></header>
      <div className="workspace-body"><nav className="activity-bar" aria-label="工作空间导航"><button className="activity-button selected" title="SSH 工作空间" aria-label="SSH 工作空间"><Icon name="terminal" size={22} /></button><button className={`activity-button ${history ? 'enabled' : ''}`} title="聊天记录" aria-label="打开聊天列表" onClick={() => setHistory(value => !value)}><Icon name="message" size={21} /></button><div className="activity-spacer" /><button className="activity-button" title="设置" aria-label="工作空间设置" onClick={() => setModal('settings')}><Icon name="settings" size={21} /></button></nav>
        <main className="workspace-main">
          {!ssh.native && <div className="preview-banner"><Icon name="info" size={15} /><span>网页界面预览 · SSH 与模型连接仅在桌面应用可用 · 当前没有连接任何服务器</span></div>}
          {ssh.error && <div className="error-banner" role="alert"><Icon name="info" size={16} /><span>{ssh.error}</span><button className="icon-button" aria-label="关闭错误提示" onClick={() => ssh.setError(null)}><Icon name="close" size={15} /></button></div>}
          <div className="workspace-grid" ref={workspace} style={{ '--terminal-share': `${split}%` } as CSSProperties}>
            <TerminalPane snapshot={ssh.snapshot} available={available} historyUnavailable={ssh.historyUnavailable} onConnect={() => { if (!active) setModal('connect'); }} onDisconnect={() => void ssh.disconnect()} subscribe={ssh.subscribeOutput} reportError={ssh.reportError} sizeRef={ssh.size} />
            <div className="pane-divider" role="separator" aria-label="调整终端与聊天的宽度" aria-orientation="vertical" aria-valuemin={40} aria-valuemax={75} aria-valuenow={split} tabIndex={0} onKeyDown={event => { if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); setSplit(value => Math.max(40, Math.min(75, value + (event.key === 'ArrowLeft' ? -2 : 2)))); } }} onPointerDown={event => { dragging.current = true; event.currentTarget.setPointerCapture(event.pointerId); }} onPointerMove={event => { if (!dragging.current || !workspace.current) return; const box = workspace.current.getBoundingClientRect(); setSplit(Math.round(Math.max(40, Math.min(75, (event.clientX - box.left) / box.width * 100)))); }} onPointerUp={event => { dragging.current = false; event.currentTarget.releasePointerCapture(event.pointerId); }} onPointerCancel={() => { dragging.current = false; }} />
            <ChatPane model={model} onConsent={requestConsent} showHistory={history} onToggleHistory={() => setHistory(value => !value)} onSettings={() => setModal('settings')} />
          </div>
        </main>
      </div>
      <footer className="app-statusbar"><span><Icon name="server" size={12} />{SESSION_LABELS[ssh.snapshot.state]}<span className="status-divider">|</span>单服务器 / 单 PTY</span><span>SSH + 模型聊天 <span className="footer-stage">M2</span></span></footer>
    </div>
    {modal === 'connect' && !ssh.challenge && <ConnectionDialog onClose={() => setModal(null)} onConnect={ssh.connect} size={ssh.size.current} available={available} />}
    {modal === 'settings' && !ssh.challenge && <SettingsDialog model={model} onConsent={() => requestConsent()} onTest={async () => { if (await model.probe()) setModal(null); }} onClose={() => setModal(null)} />}
    {consent && !ssh.challenge && <ModelConsentDialog config={consent.config} changed={consent.config.revision !== model.config.revision} busy={model.configBusy} sendAfter={!!consent.chatId} error={model.error} onConfirm={confirmConsent} onClose={() => { if (!model.configBusy) setConsent(null); }} />}
    {ssh.challenge && <HostKeyDialog key={ssh.challenge.challenge_id} challenge={ssh.challenge} onDecide={ssh.confirmHostKey} />}
  </>;
}
