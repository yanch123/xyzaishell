import { useEffect, useRef, useState } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import type { MutableRefObject } from 'react';
import type { SessionRef, SessionSnapshot, TerminalOutput, TerminalSize } from '../lib/types';
import { sameSession, SESSION_LABELS } from '../lib/types';
import { sshBridge } from '../lib/bridge';
import { fromBase64, MAX_OUTPUT_PENDING, TerminalInputQueue, terminalBytes } from '../lib/terminal';
import { Icon } from './Icon';

type Props = {
  snapshot: SessionSnapshot; available: boolean; historyUnavailable?: boolean; onConnect: () => void; onDisconnect: () => void;
  subscribe: (fn: (event: TerminalOutput) => void) => () => void;
  reportError: (error: unknown) => void; sizeRef: MutableRefObject<TerminalSize>;
};
export function TerminalPane({ snapshot, available, historyUnavailable, onConnect, onDisconnect, subscribe, reportError, sizeRef }: Props) {
  const container = useRef<HTMLDivElement>(null); const termRef = useRef<Terminal | null>(null);
  const active = useRef(snapshot); active.current = snapshot;
  const report = useRef(reportError); report.current = reportError;
  const [dimensions, setDimensions] = useState<TerminalSize>(sizeRef.current);
  const [gap, setGap] = useState<string | null>(null);
  const [hasOutput, setHasOutput] = useState(false);
  const input = useRef<TerminalInputQueue | null>(null);
  const displayedSession = useRef<SessionRef | null>(null);
  const lastOutput = useRef(0n);

  const recreate = useRef<(session: SessionRef | null) => void>(() => {});
  useEffect(() => {
    if (!container.current) return;
    let disposed = false;
    let terminal: Terminal | null = null;
    let fit: FitAddon | null = null;
    let renderedSession: SessionRef | null = null;
    let releaseInput: (() => void) | undefined;
    let resizeTimer: ReturnType<typeof setTimeout> | undefined;
    let pending = 0;
    const doFit = () => {
      if (disposed || !terminal || !fit || !container.current?.clientWidth || !container.current.clientHeight) return;
      fit.fit();
      const size = { cols: Math.max(1, Math.min(1000, terminal.cols)), rows: Math.max(1, Math.min(1000, terminal.rows)) };
      sizeRef.current = size; setDimensions(size);
      clearTimeout(resizeTimer);
      const captured = active.current.session;
      if (captured && active.current.state === 'connected') resizeTimer = setTimeout(() => {
        if (!sameSession(captured, active.current.session) || active.current.state !== 'connected') return;
        void sshBridge.resize(captured, size).catch(failure => { if (!disposed && sameSession(captured, active.current.session)) report.current(failure); });
      }, 80);
    };
    const createTerminal = (session: SessionRef | null) => {
      if (terminal && ((session === null && renderedSession === null) || sameSession(session, renderedSession))) return;
      // reset() does not clear xterm's asynchronous WriteBuffer. A new generation
      // must own a fresh terminal, so queued escape sequences cannot cross hosts.
      releaseInput?.(); terminal?.dispose(); clearTimeout(resizeTimer);
      container.current?.replaceChildren();
      renderedSession = session; displayedSession.current = session;
      lastOutput.current = 0n; pending = 0; setGap(null); setHasOutput(false);
      const instance = new Terminal({
        cursorBlink: true, cursorStyle: 'bar', fontSize: 14, lineHeight: 1.3,
        fontFamily: '"Cascadia Code", "SFMono-Regular", Consolas, "Liberation Mono", monospace',
        scrollback: 5000, convertEol: false, disableStdin: active.current.state !== 'connected', allowProposedApi: false,
        logLevel: 'off', linkHandler: { activate: () => {}, allowNonHttpProtocols: false },
        theme: { background: '#101217', foreground: '#d2d8e3', cursor: '#a8c5ff', selectionBackground: '#43577688', black: '#27303e', red: '#f38ba8', green: '#a6d7a1', yellow: '#edcc8a', blue: '#8cb4fa', magenta: '#c6a0f6', cyan: '#85d8db', white: '#d2d8e3', brightBlack: '#738096', brightRed: '#ffadc2', brightGreen: '#bce7b8', brightYellow: '#f4dca7', brightBlue: '#b1ccff', brightMagenta: '#d6b8ff', brightCyan: '#a8eaed', brightWhite: '#f4f7fb' },
      });
      const addon = new FitAddon(); instance.loadAddon(addon);
      instance.open(container.current!); terminal = instance; fit = addon; termRef.current = instance;
      // Built-in OSC 8 navigation is inert; no clipboard/link addons are enabled.
      // Each input callback is fenced to its terminal generation.
      const canInput = () => !disposed && terminal === instance && active.current.state === 'connected' && sameSession(session, active.current.session);
      const data = instance.onData(value => { if (canInput()) input.current?.enqueue(terminalBytes(value)); });
      const binary = instance.onBinary(value => { if (canInput()) input.current?.enqueue(terminalBytes(value, true)); });
      releaseInput = () => { data.dispose(); binary.dispose(); };
      doFit();
    };
    recreate.current = createTerminal;
    createTerminal(active.current.session);
    const observer = new ResizeObserver(doFit); observer.observe(container.current);
    const frame = requestAnimationFrame(doFit);
    const unsubscribe = subscribe(event => {
      if (disposed) return;
      createTerminal(event.session);
      const instance = terminal!;
      const sequence = BigInt(event.seq);
      if (sequence <= lastOutput.current) return;
      if (sequence !== lastOutput.current + 1n) setGap('终端输出存在缺口，当前显示不是完整历史。请自行核实远端状态。');
      lastOutput.current = sequence;
      let bytes: Uint8Array;
      try { bytes = fromBase64(event.data_base64); }
      catch { setGap('收到无法解码的终端数据，当前输出不完整。'); return; }
      if (pending + bytes.byteLength > MAX_OUTPUT_PENDING) {
        setGap('终端渲染队列超过上限，部分输出未显示。请断开后核实远端状态，不要重放旧命令。');
        return;
      }
      pending += bytes.byteLength; setHasOutput(true);
      instance.write(bytes, () => {
        if (disposed || terminal !== instance) return;
        pending -= bytes.byteLength;
        void sshBridge.ackOutput(event.session, event.seq).catch(failure => {
          if (!disposed && active.current.state === 'connected' && sameSession(event.session, active.current.session)) report.current(failure);
        });
      });
    });
    return () => { disposed = true; recreate.current = () => {}; cancelAnimationFrame(frame); clearTimeout(resizeTimer); observer.disconnect(); unsubscribe(); releaseInput?.(); terminal?.dispose(); termRef.current = null; };
  }, [subscribe, sizeRef]);

  useEffect(() => {
    input.current?.dispose(); input.current = null;
    if (snapshot.session) recreate.current(snapshot.session);
    const terminal = termRef.current;
    if (terminal) terminal.options.disableStdin = snapshot.state !== 'connected';
    if (snapshot.state === 'connected' && snapshot.session) {
      const captured = snapshot.session;
      input.current = new TerminalInputQueue(captured, sshBridge.write, failure => { if (sameSession(captured, active.current.session)) report.current(failure); });
      terminal?.focus();
      void sshBridge.resize(captured, sizeRef.current).catch(failure => { if (sameSession(captured, active.current.session)) report.current(failure); });
    }
    return () => { input.current?.dispose(); input.current = null; };
  }, [snapshot.session?.session_id, snapshot.session?.generation, snapshot.state, sizeRef]);

  useEffect(() => { if (historyUnavailable) setGap('之前的终端历史未恢复。若输出不再更新，请明确断开后重新连接，不要重放旧命令。'); }, [historyUnavailable]);

  const online = snapshot.state === 'connected';
  const busy = ['connecting', 'awaiting_host_key', 'authenticating', 'closing'].includes(snapshot.state);
  return <section className="terminal-pane" aria-label="SSH 终端">
    <div className="pane-toolbar"><div className="pane-tab"><Icon name="terminal" size={16} /><span>{snapshot.host ? `${snapshot.username}@${snapshot.host}` : 'SSH 终端'}</span><span className={`status-dot ${online ? 'online' : busy ? 'waiting' : ''}`} /></div><div className="toolbar-actions"><span className="protocol-tag">SSH</span>{online && <button className="terminal-interrupt" title="明确发送 Ctrl+C 到当前终端；不停止 AI 流程，也不保证远端程序被中断" onClick={() => input.current?.enqueue(terminalBytes('\x03'))}>发送 Ctrl+C</button>}{online || busy ? <button className="icon-button" aria-label="断开 SSH 连接" title="断开 SSH 连接，不保证远端程序已停止" disabled={snapshot.state === 'closing'} onClick={onDisconnect}><Icon name="disconnect" size={16} /></button> : <button className="icon-button" aria-label="新建 SSH 连接" title="新建 SSH 连接" onClick={onConnect} disabled={!available}><Icon name="plus" size={17} /></button>}</div></div>
    {gap && <div className="terminal-warning" role="alert"><Icon name="info" size={15} />{gap}</div>}
    <div className="terminal-stage"><div className={`xterm-container ${!online && !hasOutput ? 'quiet' : ''}`} ref={container} />
      {!online && !hasOutput && <div className="terminal-empty"><div className="terminal-glyph"><Icon name="terminal" size={34} /></div><div className="eyebrow">YOUR REMOTE WORKSPACE</div><h1>{busy ? SESSION_LABELS[snapshot.state] : '从一条安全连接开始'}</h1><p>{busy ? '正在建立可见的 SSH 终端。连接成功后即可手动输入。' : '熟悉的终端，专注的工作空间。\n连接你的服务器，让每一步操作清晰可见。'}</p>{!busy && <button className="button primary connect-hero" onClick={onConnect} disabled={!available}><Icon name="plus" size={17} />连接服务器</button>}<div className="empty-footnote"><Icon name="shield" size={14} /><span>{available ? '主机身份核验 · 凭据仅用于本次连接' : '当前为网页预览，SSH 需在桌面应用中使用'}</span></div></div>}
    </div>
    <div className="terminal-status"><span><span className={`status-dot ${online ? 'online' : busy ? 'waiting' : ''}`} />{SESSION_LABELS[snapshot.state]}</span><span>{online && snapshot.port ? `端口 ${snapshot.port} · ` : ''}UTF-8<span className="status-divider">|</span>{dimensions.cols} × {dimensions.rows}</span></div>
  </section>;
}
