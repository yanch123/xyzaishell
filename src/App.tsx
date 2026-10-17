import { useCallback, useEffect, useRef, useState } from 'react';
import type { CSSProperties } from 'react';
import { ShellLogDialog } from './components/ShellLogDialog';
import type { ShellLogScope } from './components/ShellLogDialog';
import { historyBridge, storageBridge } from './lib/storageBridge';
import type { HistoryStatus } from './lib/storageBridge';
import { windowBridge } from './lib/windowBridge';
import { sshBridge } from './lib/bridge';
import { ProfileDialog } from './components/ProfileDialog';
import { useTerminalAgent } from './hooks/useTerminalAgent';
import { useShellIntegration } from './hooks/useShellIntegration';
import { IntegrationPanel, INTEGRATION_LABELS } from './components/IntegrationPanel';
import type { ModelConfigStatus } from './lib/model';
import type { SessionSnapshot } from './lib/types';
import { sameSession, safeError, isActiveSession, SESSION_LABELS } from './lib/types';
import { useModelChat } from './hooks/useModelChat';
import { ModelConsentDialog } from './components/ModelConsentDialog';
import { useSshSession } from './hooks/useSshSession';
import { TerminalPane } from './components/TerminalPane';
import { ChatPane } from './components/ChatPane';
import { ConnectionDialog } from './components/ConnectionDialog';
import { HostKeyDialog } from './components/HostKeyDialog';
import { SettingsDialog } from './components/SettingsDialog';
import { Modal } from './components/Modal';
import { NonModalPanel } from './components/NonModalPanel';
import { Icon } from './components/Icon';

type Inspector = 'profiles' | 'settings' | 'shell-log' | 'help' | 'integration' | null;
type WorkspaceMessage = { id: string; source: string; level: 'info' | 'error' | 'warning'; text: string; at: number };
type SavedOptions = { temporaryCredential?: string; rememberCredential?: boolean };
type SwitchDecision = { title: string; destination?: string; target: SessionSnapshot; proceed: () => Promise<boolean>; resolve: (accepted: boolean) => void; reject: (failure: unknown) => void };

export default function App() {
  const ssh = useSshSession();
  const model = useModelChat(ssh.snapshot);
  const agent = useTerminalAgent(model.config, ssh.snapshot);
  useEffect(() => { if (model.terminalTask) agent.accept(model.terminalTask); }, [model.terminalTask, agent.accept]);
  const integration = useShellIntegration(ssh.native, ssh.snapshot);
  const [historyStatus, setHistoryStatus] = useState<HistoryStatus | null>(null);
  const [inspector, setInspectorState] = useState<Inspector>(null);
  const [openedInspectors, setOpenedInspectors] = useState<Inspector[]>([]);
  const setInspector = useCallback((next: Inspector) => {
    setInspectorState(next);
    if (next) setOpenedInspectors(previous => previous.includes(next) ? previous : [...previous, next]);
  }, []);
  const closeInspector = (closed: Inspector) => {
    setOpenedInspectors(previous => previous.filter(item => item !== closed));
    setInspectorState(current => current === closed ? null : current);
  };
  const [connecting, setConnecting] = useState(false);
  const [initialProfileId, setInitialProfileId] = useState<string | undefined>();
  const [initialTarget, setInitialTarget] = useState<{ host: string; port: number; username: string; auth_kind: 'password' } | undefined>();
  const [logScope, setLogScope] = useState<ShellLogScope>({ kind: 'all' });
  const [history, setHistory] = useState(false);
  const [messagesOpen, setMessagesOpen] = useState(false);
  const [messages, setMessages] = useState<WorkspaceMessage[]>([]);
  const [windowBusy, setWindowBusy] = useState(false);
  const windowLock = useRef(false);
  const [switchDecision, setSwitchDecision] = useState<SwitchDecision | null>(null);
  const [switchBusy, setSwitchBusy] = useState(false);
  const [switchError, setSwitchError] = useState<string | null>(null);
  const switchLock = useRef(false);
  const pendingSwitch = useRef<SwitchDecision | null>(null);
  const [split, setSplit] = useState(64);
  const workspace = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);
  const available = ssh.native && ssh.ready;
  const active = isActiveSession(ssh.snapshot.state);
  const addMessage = useCallback((id: string, source: string, text: string, level: WorkspaceMessage['level'] = 'info') => {
    setMessages(previous => [{ id, source, text, level, at: Date.now() }, ...previous.filter(item => item.id !== id)].slice(0, 50));
  }, []);
  useEffect(() => {
    if (!ssh.native) return;
    let live = true; let unlisten: (() => void) | undefined;
    void historyBridge.listen(next => { if (live) setHistoryStatus(next); }).then(stop => { if (live) unlisten = stop; else stop(); }).catch(() => {});
    void historyBridge.status().then(next => { if (live) setHistoryStatus(next); }).catch(() => {});
    return () => { live = false; unlisten?.(); };
  }, [ssh.native]);
  useEffect(() => {
    if (!available) return;
    let live = true;
    void windowBridge.target().then(target => {
      if (!live) return;
      if (target.profile_id) { setInitialProfileId(target.profile_id); setInspector('profiles'); }
      else if (target.new_connection) setConnecting(true);
    }).catch(failure => { if (live) addMessage('window-target', '连接', safeError(failure), 'error'); });
    return () => { live = false; };
  }, [available, addMessage]);
  useEffect(() => { if (ssh.error) addMessage('connection-error', '连接', ssh.error, 'error'); }, [ssh.error, addMessage]);
  useEffect(() => { if (historyStatus?.error) addMessage('history-error', '历史', historyStatus.error.safe_message, 'error'); }, [historyStatus?.error, addMessage]);
  useEffect(() => { if (model.error) addMessage('model-error', '模型', model.error, 'error'); }, [model.error, addMessage]);
  useEffect(() => {
    if (ssh.snapshot.state === 'connected') addMessage(`session-${ssh.snapshot.session?.session_id}`, '连接', `已连接 ${ssh.snapshot.username}@${ssh.snapshot.host}:${ssh.snapshot.port ?? 22}`);
  }, [ssh.snapshot.state, ssh.snapshot.session?.session_id, ssh.snapshot.host, ssh.snapshot.port, ssh.snapshot.username, addMessage]);
  const configSeen = useRef<string | null>(null);
  useEffect(() => {
    if (configSeen.current && model.config.revision !== configSeen.current) addMessage('model-settings', '模型', model.config.ready ? `模型设置已更新${model.config.consented ? '' : '，发送前需确认接收方'}` : '模型配置尚不完整，手动 SSH 仍可使用');
    configSeen.current = model.config.revision;
  }, [model.config.revision, model.config.ready, model.config.consented, addMessage]);
  useEffect(() => () => { pendingSwitch.current?.resolve(false); pendingSwitch.current = null; }, []);
  const launchWindow = async (profileId?: string): Promise<boolean> => {
    if (!available || windowLock.current) return false;
    windowLock.current = true; setWindowBusy(true);
    try { await windowBridge.launch(profileId ?? null); addMessage('new-window', '连接', '已启动独立进程窗口，请在新窗口核对目标并连接。'); return true; }
    catch (failure) { addMessage('new-window', '连接', safeError(failure), 'error'); return false; }
    finally { windowLock.current = false; setWindowBusy(false); }
  };
  const openNewConnection = () => { if (active) void launchWindow(); else { setInitialTarget(undefined); setConnecting(true); } };
  const openProfiles = (id?: string) => { setInitialProfileId(id); setInspector('profiles'); };
  const openLogs = (scope: ShellLogScope) => { setLogScope(scope); setInspector('shell-log'); };
  const connectSaved = (id: string, revision: string, options?: SavedOptions) => active ? launchWindow(id) : ssh.connectSaved(id, revision, options);
  const requestSwitch = (title: string, proceed: () => Promise<boolean>, destination?: string): Promise<boolean> => {
    if (pendingSwitch.current || switchLock.current) return Promise.resolve(false);
    return new Promise((resolve, reject) => { const value = { title, destination, target: ssh.snapshot, proceed, resolve, reject }; pendingSwitch.current = value; setSwitchError(null); setSwitchDecision(value); });
  };
  const switchSaved = async (id: string, revision: string, options?: SavedOptions) => {
    if (!active) return ssh.connectSaved(id, revision, options);
    const listing = await storageBridge.profiles();
    const target = listing.listing.connections.find(profile => profile.id === id);
    if (!target || listing.listing.revision !== revision) throw { code: 'PROFILE_CHANGED', safe_message: '连接档案已变化，请重新读取并核对目标后再切换。' };
    return requestSwitch('在当前窗口切换连接', () => ssh.connectSaved(id, revision, options), `${target.label} · ${target.username}@${target.host}:${target.port}`);
  };
  const cancelSwitch = () => { if (switchLock.current) return; pendingSwitch.current?.resolve(false); pendingSwitch.current = null; setSwitchDecision(null); setSwitchError(null); };
  const confirmSwitch = async () => {
    const decision = pendingSwitch.current;
    if (!decision || switchLock.current) return;
    switchLock.current = true; setSwitchBusy(true); setSwitchError(null);
    try {
      const current = await sshBridge.snapshot();
      if (isActiveSession(current.state) && !sameSession(decision.target.session, current.session)) throw { code: 'SESSION_CHANGED', safe_message: '当前连接已变化，请取消后重新核对切换目标。' };
      if (agent.owner) await agent.cancel(agent.owner.task_id, agent.owner.run_id);
      await Promise.all(model.chats.filter(chat => chat.active_run_id).map(chat => model.cancel(chat.chat_id)));
      if (isActiveSession(current.state) && current.session) {
        await ssh.disconnect();
        const deadline = Date.now() + 10000;
        while (true) {
          const next = await sshBridge.snapshot();
          if (!isActiveSession(next.state)) break;
          if (!sameSession(current.session, next.session)) throw { code: 'SESSION_CHANGED', safe_message: '连接已变化，已取消后续切换。' };
          if (Date.now() >= deadline) throw { code: 'DISCONNECT_PENDING', safe_message: '尚未确认旧连接已断开，未发起新连接。请核对状态后重新选择。' };
          await new Promise(resolve => setTimeout(resolve, 100));
        }
      }
      const accepted = await decision.proceed();
      decision.resolve(accepted); pendingSwitch.current = null; setSwitchDecision(null);
      if (!accepted) addMessage('switch', '连接', '新连接请求未完成；旧连接不会自动恢复，请核对当前状态。', 'warning');
    } catch (failure) { addMessage('switch', '连接', safeError(failure), 'error'); decision.reject(failure); pendingSwitch.current = null; setSwitchDecision(null); }
    finally { switchLock.current = false; setSwitchBusy(false); }
  };
  const reconnect = () => {
    if (!available || !ssh.snapshot.host || !ssh.snapshot.username) return;
    void requestSwitch('重新连接当前目标', async () => {
      if (ssh.snapshot.profile_id) {
        const id = ssh.snapshot.profile_id;
        const profiles = await storageBridge.profiles();
        if (!profiles.listing.connections.some(profile => profile.id === id)) throw { code: 'PROFILE_REMOVED', safe_message: '原连接档案已被删除，旧连接已断开。请选择其他连接或新建连接。' };
        try { return await ssh.connectSaved(id, profiles.listing.revision); }
        catch (failure) {
          const code = failure && typeof failure === 'object' && 'code' in failure ? failure.code : null;
          if (typeof code !== 'string' || !code.startsWith('CREDENTIAL_')) throw failure;
          openProfiles(id); addMessage('reconnect-auth', '连接', '旧连接已断开。请为所选连接提供本次认证资料。', 'warning');
        }
      } else { setInitialTarget({ host: ssh.snapshot.host!, port: ssh.snapshot.port ?? 22, username: ssh.snapshot.username!, auth_kind: 'password' }); setConnecting(true); }
      return true;
    }).catch(failure => addMessage('reconnect', '连接', safeError(failure), 'error'));
  };
  const [consent, setConsent] = useState<{ config: ModelConfigStatus; target: SessionSnapshot; chatId?: string } | null>(null);
  const requestConsent = (chatId?: string) => setConsent({ config: model.config, target: ssh.snapshot, chatId: chatId && !model.attachments[chatId] ? chatId : undefined });
  const confirmConsent = async () => {
    if (!consent || !await model.confirmDestination(consent.config, consent.target)) return;
    const chatId = consent.chatId; setConsent(null); if (chatId) await model.send(chatId);
  };
  const hasModal = connecting || !!ssh.challenge || !!consent || !!switchDecision;
  const pendingCount = Number(!!ssh.challenge) + Number(!!consent) + Number(!!switchDecision) + Number(!!agent.owner && ['waiting_approval', 'needs_user', 'needs_secrets', 'unsupported_action', 'budget_paused'].includes(agent.owner.phase));
  const targetLabel = ssh.snapshot.host ? `${ssh.snapshot.username ?? ''}@${ssh.snapshot.host}:${ssh.snapshot.port ?? 22}` : '未连接服务器';
  const closeMenu = (event: React.MouseEvent<HTMLDivElement>) => { if ((event.target as HTMLElement).closest('button')) event.currentTarget.closest('details')?.removeAttribute('open'); };
  return <>
    <div className="app-shell" inert={hasModal}>
      <header className="app-header"><div className="brand"><div className="brand-mark"><Icon name="terminal" size={19} /></div><span>xyzai<span className="brand-light">shell</span></span><span className="version-tag">v0.2.1</span></div><div className="header-workspace"><span className={`status-dot ${ssh.snapshot.state === 'connected' ? 'online' : ''}`} /><span>{targetLabel}</span><span className="workspace-divider">/</span><span>SSH 工作空间</span></div><button className="header-settings icon-button" aria-label="打开设置" title="工作空间设置" onClick={() => setInspector('settings')}><Icon name="settings" size={19} /></button></header>
      <nav className="desktop-menubar" aria-label="应用菜单">
        <details><summary>连接</summary><div className="desktop-menu" onClick={closeMenu}><button onClick={openNewConnection} disabled={windowBusy}>新建连接</button><button onClick={() => openProfiles()}>已保存连接</button><button onClick={() => void launchWindow()} disabled={!available || windowBusy}>新窗口</button><button onClick={reconnect} disabled={!available || !ssh.snapshot.host || switchBusy}>重新连接</button><button onClick={() => void ssh.disconnect()} disabled={!active}>断开</button><button onClick={() => openProfiles()} disabled={!active}>在当前窗口切换…</button></div></details>
        <details><summary>查看</summary><div className="desktop-menu" onClick={closeMenu}><button onClick={() => openProfiles()}>连接列表</button><button onClick={() => setMessagesOpen(value => !value)}>消息面板</button><button onClick={() => openLogs({ kind: 'all' })}>操作日志</button><button onClick={() => setHistory(value => !value)}>聊天历史</button></div></details>
        <details><summary>工具</summary><div className="desktop-menu" onClick={closeMenu}><button onClick={() => setInspector('settings')}>设置</button><button onClick={() => setInspector('integration')}>命令状态识别</button></div></details>
        <details><summary>帮助</summary><div className="desktop-menu" onClick={closeMenu}><button onClick={() => setInspector('help')}>使用说明</button><button onClick={() => setInspector('settings')}>配置与凭据帮助</button><button onClick={() => setInspector('help')}>关于 xyzaishell</button></div></details>
      </nav>
      <div className="desktop-toolbar" aria-label="常用操作"><button className="button secondary" disabled={windowBusy} onClick={openNewConnection}><Icon name="plus" size={14} />新建连接</button><button className="button secondary" disabled={!available} onClick={() => active ? void ssh.disconnect() : setConnecting(true)}><Icon name={active ? 'disconnect' : 'link'} size={14} />{active ? '断开' : '连接'}</button><button className="button secondary" onClick={() => openProfiles()}>连接管理</button><button className="button secondary" onClick={() => setInspector('settings')}>设置</button><button className="button secondary" onClick={() => openLogs({ kind: 'current' })}>当前终端日志</button><span className="toolbar-spacer" /><button className="button secondary" title={integration.status?.error?.safe_message ?? undefined} onClick={() => setInspector('integration')}>{integration.status ? INTEGRATION_LABELS[integration.status.phase] : '命令状态识别'}</button><button className="button secondary" aria-expanded={messagesOpen} onClick={() => setMessagesOpen(value => !value)}>消息 {messages.length}{pendingCount > 0 ? ` · ${pendingCount} 待决定` : ''}</button></div>
      <div className="workspace-body"><nav className="activity-bar" aria-label="工作空间导航"><button className="activity-button selected" title="SSH 工作空间" aria-label="SSH 工作空间"><Icon name="terminal" size={22} /></button><button className="activity-button" title="连接管理" aria-label="打开连接管理" onClick={() => openProfiles()}><Icon name="server" size={21} /></button><button className={`activity-button ${history ? 'enabled' : ''}`} title="聊天记录" aria-label="打开聊天列表" onClick={() => setHistory(value => !value)}><Icon name="message" size={21} /></button><button className="activity-button" title="终端日志" aria-label="打开终端日志" onClick={() => openLogs({ kind: 'current' })}><Icon name="terminal" size={21} /></button><div className="activity-spacer" /><button className="activity-button" title="设置" aria-label="工作空间设置" onClick={() => setInspector('settings')}><Icon name="settings" size={21} /></button></nav>
        <main className="workspace-main">
          {!ssh.native && <div className="preview-banner"><Icon name="info" size={15} /><span>网页界面预览 · SSH 与模型连接仅在桌面应用可用 · 当前没有连接任何服务器</span></div>}
          {ssh.error && <div className="error-banner" role="alert"><Icon name="info" size={16} /><span>{ssh.error}</span><button className="icon-button" aria-label="关闭错误提示" onClick={() => ssh.setError(null)}><Icon name="close" size={15} /></button></div>}
          <div className="workspace-grid" ref={workspace} style={{ '--terminal-share': `${split}%` } as CSSProperties}>
            <TerminalPane snapshot={ssh.snapshot} available={available} historyUnavailable={ssh.historyUnavailable} onConnect={() => { if (!active) setConnecting(true); }} onDisconnect={() => void ssh.disconnect()} subscribe={ssh.subscribeOutput} reportError={ssh.reportError} sizeRef={ssh.size} onAttachSelection={attachment => model.current ? model.attachSelection(model.current.chat_id, attachment) : false} />
            <div className="pane-divider" role="separator" aria-label="调整终端与聊天的宽度" aria-orientation="vertical" aria-valuemin={40} aria-valuemax={75} aria-valuenow={split} tabIndex={0} onKeyDown={event => { if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); setSplit(value => Math.max(40, Math.min(75, value + (event.key === 'ArrowLeft' ? -2 : 2)))); } }} onPointerDown={event => { dragging.current = true; event.currentTarget.setPointerCapture(event.pointerId); }} onPointerMove={event => { if (!dragging.current || !workspace.current) return; const box = workspace.current.getBoundingClientRect(); setSplit(Math.round(Math.max(40, Math.min(75, (event.clientX - box.left) / box.width * 100)))); }} onPointerUp={event => { dragging.current = false; event.currentTarget.releasePointerCapture(event.pointerId); }} onPointerCancel={() => { dragging.current = false; }} />
            <ChatPane historyStatus={historyStatus} agent={agent} session={ssh.snapshot} displayEnabled={!hasModal} model={model} onConsent={requestConsent} showHistory={history} onToggleHistory={() => setHistory(value => !value)} onSettings={() => setInspector('settings')} />
          </div>
        </main>
      </div>
      <section className="workspace-notices" aria-label="工作空间消息"><button aria-expanded={messagesOpen} onClick={() => setMessagesOpen(value => !value)}>{messagesOpen ? '收起' : '展开'}消息 · {pendingCount ? `${pendingCount} 项待决定 · ` : ''}{messages[0]?.text ?? '就绪。配置模型不会影响手动 SSH。'}</button>{messagesOpen && <div className="workspace-message-list">{pendingCount > 0 && <div className="workspace-message warning">待决定事项保留在原入口，阅读或关闭消息不会批准任何操作。{agent.owner && <button onClick={() => { model.selectChat(agent.owner!.chat_id); setInspector(null); }}>查看 AI 任务</button>}</div>}{messages.map(item => <div key={item.id} className={`workspace-message ${item.level}`}><time dateTime={new Date(item.at).toISOString()}>{new Date(item.at).toLocaleTimeString()}</time><span>{item.source} · {item.text}</span></div>)}</div>}</section>
      <footer className="app-statusbar"><span><Icon name="server" size={12} />{SESSION_LABELS[ssh.snapshot.state]}</span><span>{model.config.ready ? model.config.consented ? '模型可用' : '模型待确认' : '模型未配置'} · <button onClick={() => setMessagesOpen(true)}>{messages.length} 条消息</button> <span className="footer-stage">v0.2.1</span></span></footer>
    </div>
    <div inert={hasModal}>
      {openedInspectors.includes('shell-log') && <div hidden={inspector !== 'shell-log'} inert={inspector !== 'shell-log'}><ShellLogDialog key={JSON.stringify(logScope) + (logScope.kind === 'current' ? ssh.snapshot.session?.session_id ?? 'none' : '')} native={ssh.native} snapshot={ssh.snapshot} initialScope={logScope} integration={integration.status} onMessage={summary => addMessage('log-export', '日志', summary)} onClose={() => closeInspector('shell-log')} /></div>}
      {openedInspectors.includes('profiles') && <div hidden={inspector !== 'profiles'} inert={inspector !== 'profiles'}><ProfileDialog available={available} activeSession={active} initialProfileId={initialProfileId} onClose={() => closeInspector('profiles')} onConnect={connectSaved} onConnectInNewWindow={launchWindow} onSwitchCurrent={switchSaved} onNewConnection={openNewConnection} onViewLogs={profile => openLogs({ kind: 'profile', id: profile.id, label: profile.label })} /></div>}
      {openedInspectors.includes('settings') && <div hidden={inspector !== 'settings'} inert={inspector !== 'settings'}><SettingsDialog onHistoryStatus={setHistoryStatus} model={model} onConsent={() => requestConsent()} onTest={async () => { await model.probe(); }} onClose={() => closeInspector('settings')} /></div>}
      {openedInspectors.includes('integration') && <div hidden={inspector !== 'integration'} inert={inspector !== 'integration'}><IntegrationPanel integration={integration} snapshot={ssh.snapshot} onClose={() => closeInspector('integration')} /></div>}
      {openedInspectors.includes('help') && <div hidden={inspector !== 'help'} inert={inspector !== 'help'}><NonModalPanel title="xyzaishell v0.2.1" eyebrow="SSH + AI 工作空间" onClose={() => closeInspector('help')} className="help-panel"><p>一个窗口对应一个进程、一条 SSH 会话和一个 PTY。已有连接时，新建连接会启动独立进程；在当前窗口切换需要明确确认。启动失败不会断开原窗口。</p><p>手动 SSH 不依赖模型。设置、连接管理和操作日志不会锁住终端。普通命令自动继续依赖当前 Shell 集成；危险命令仍需批准，状态未知时先核对。</p><p>模型只接收明确发送的内容。请勿在聊天中输入密码或密钥。历史恢复仅供阅读，不恢复执行或旧批准；日志可能含敏感明文，必须明确启用。</p><button className="button secondary" onClick={() => setInspector('settings')}>查看配置与凭据帮助</button></NonModalPanel></div>}
    </div>
    {connecting && !ssh.challenge && <ConnectionDialog initialTarget={initialTarget} onConnectSaved={connectSaved} onProfiles={() => { setConnecting(false); openProfiles(); }} onClose={() => setConnecting(false)} onConnect={ssh.connect} size={ssh.size.current} available={available} />}
    {consent && !ssh.challenge && <ModelConsentDialog config={consent.config} target={consent.target} changed={consent.config.revision !== model.config.revision || consent.config.destination !== model.config.destination || (!!consent.target.session || !!ssh.snapshot.session) && !sameSession(consent.target.session, ssh.snapshot.session) || consent.target.state !== ssh.snapshot.state} busy={model.configBusy} sendAfter={!!consent.chatId} error={model.error} onConfirm={confirmConsent} onClose={() => { if (!model.configBusy) setConsent(null); }} />}
    {switchDecision && !ssh.challenge && <Modal title={switchDecision.title} eyebrow="当前窗口连接将被中断" onClose={cancelSwitch}><p className="modal-intro">{switchDecision.destination && <>新目标：{switchDecision.destination}。<br /></>}当前目标：{switchDecision.target.username}@{switchDecision.target.host}:{switchDecision.target.port ?? 22}。继续会请求停止当前 AI 生成和任务，再断开此 SSH；已发送到远端的程序不保证停止。新连接失败不会自动恢复旧会话，也不会重放命令或批准。</p>{switchError && <p className="inline-error" role="alert">{switchError}</p>}<div className="modal-footer"><button className="button secondary" disabled={switchBusy} onClick={cancelSwitch}>取消</button><button className="button primary" disabled={switchBusy || !!switchError} onClick={() => void confirmSwitch()}>{switchBusy ? '正在断开并核对…' : '停止 AI 并继续'}</button></div></Modal>}
    {ssh.challenge && <HostKeyDialog key={ssh.challenge.challenge_id} challenge={ssh.challenge} onDecide={ssh.confirmHostKey} />}
  </>;
}
