import { useEffect, useRef, useState } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { SearchAddon } from '@xterm/addon-search';
import '@xterm/xterm/css/xterm.css';
import type { MutableRefObject } from 'react';
import type { SessionRef, SessionSnapshot, TerminalOutput, TerminalSize } from '../lib/types';
import { sameSession, SESSION_LABELS } from '../lib/types';
import { sshBridge } from '../lib/bridge';
import { fromBase64, MAX_OUTPUT_PENDING, TerminalInputQueue, terminalBytes } from '../lib/terminal';
import { Icon } from './Icon';
import { useTerminalSettings } from '../hooks/useTerminalSettings';
import { captureSelection } from '../lib/terminalSelection';
import type { TerminalSelection } from '../lib/terminalSelection';
import './terminal-tools.css';

const SEARCH_DECORATIONS = { matchBackground: '#514825', matchOverviewRuler: '#a89540', activeMatchBackground: '#7b641f', activeMatchBorder: '#e9c55b', activeMatchColorOverviewRuler: '#e9c55b' };
const MAX_CLIPBOARD_BYTES = 16384;
const unsafePasteControls = (text: string) => /[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f]/.test(text);

type Props = {
  snapshot: SessionSnapshot; available: boolean; historyUnavailable?: boolean; onConnect: () => void; onDisconnect: () => void;
  subscribe: (fn: (event: TerminalOutput) => void) => () => void;
  reportError: (error: unknown) => void; sizeRef: MutableRefObject<TerminalSize>;
  onAttachSelection?: (selection: TerminalSelection) => boolean;
};
export function TerminalPane({ snapshot, available, historyUnavailable, onConnect, onDisconnect, subscribe, reportError, sizeRef, onAttachSelection }: Props) {
  const { settings } = useTerminalSettings();
  const preferences = useRef(settings); preferences.current = settings;
  const fitRef = useRef<() => void>(() => {});
  const searchRef = useRef<SearchAddon | null>(null);
  const searchInput = useRef<HTMLInputElement>(null);
  const [searchOpen, setSearchOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [results, setResults] = useState({ resultIndex: -1, resultCount: 0 });
  const [selected, setSelected] = useState(false);
  const [toolMessage, setToolMessage] = useState<string | null>(null);
  const [paste, setPaste] = useState<{ text: string; session: SessionRef } | null>(null);
  const [clipboardBusy, setClipboardBusy] = useState(false);
  const clipboardLock = useRef(false);
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
    fitRef.current = doFit;
    const createTerminal = (session: SessionRef | null) => {
      if (terminal && ((session === null && renderedSession === null) || sameSession(session, renderedSession))) return;
      // reset() does not clear xterm's asynchronous WriteBuffer. A new generation
      // must own a fresh terminal, so queued escape sequences cannot cross hosts.
      releaseInput?.(); terminal?.dispose(); clearTimeout(resizeTimer);
      container.current?.replaceChildren();
      renderedSession = session; displayedSession.current = session;
      lastOutput.current = 0n; pending = 0; setGap(null); setHasOutput(false); setSelected(false); setQuery(''); setResults({ resultIndex: -1, resultCount: 0 }); setPaste(null); setToolMessage(null);
      const instance = new Terminal({
        cursorBlink: true, cursorStyle: 'bar', fontSize: preferences.current.fontSize, lineHeight: 1.3,
        fontFamily: '"Cascadia Code", "SFMono-Regular", Consolas, "Liberation Mono", monospace',
        scrollback: preferences.current.scrollback, convertEol: false, disableStdin: active.current.state !== 'connected', allowProposedApi: true,
        logLevel: 'off', linkHandler: { activate: () => {}, allowNonHttpProtocols: false },
        theme: { background: '#101217', foreground: '#d2d8e3', cursor: '#a8c5ff', selectionBackground: '#43577688', black: '#27303e', red: '#f38ba8', green: '#a6d7a1', yellow: '#edcc8a', blue: '#8cb4fa', magenta: '#c6a0f6', cyan: '#85d8db', white: '#d2d8e3', brightBlack: '#738096', brightRed: '#ffadc2', brightGreen: '#bce7b8', brightYellow: '#f4dca7', brightBlue: '#b1ccff', brightMagenta: '#d6b8ff', brightCyan: '#a8eaed', brightWhite: '#f4f7fb' },
      });
      const addon = new FitAddon(); instance.loadAddon(addon);
      instance.open(container.current!); terminal = instance; fit = addon; termRef.current = instance;
      // Search decorations use xterm's proposed local rendering API only.
      const search = new SearchAddon({ highlightLimit: 1000 }); instance.loadAddon(search); searchRef.current = search;
      const searchResult = search.onDidChangeResults(value => { if (!disposed && terminal === instance) setResults(value); });
      const selection = instance.onSelectionChange(() => { if (!disposed && terminal === instance) setSelected(instance.hasSelection()); });
      instance.attachCustomKeyEventHandler(event => {
        if ((event.ctrlKey || event.metaKey) && event.shiftKey && !event.altKey && event.key.toLowerCase() === 'f') {
          if (event.type === 'keydown') { event.preventDefault(); setSearchOpen(true); requestAnimationFrame(() => searchInput.current?.focus()); }
          return false;
        }
        // Shell Ctrl+F and Ctrl+C retain their normal xterm behavior.
        return true;
      });
      // Built-in OSC 8 navigation is inert; no clipboard/link addons are enabled.
      // Each input callback is fenced to its terminal generation.
      const canInput = () => !disposed && terminal === instance && active.current.state === 'connected' && sameSession(session, active.current.session);
      const data = instance.onData(value => { if (canInput()) input.current?.enqueue(terminalBytes(value)); });
      const binary = instance.onBinary(value => { if (canInput()) input.current?.enqueue(terminalBytes(value, true)); });
      releaseInput = () => { data.dispose(); binary.dispose(); searchResult.dispose(); selection.dispose(); };
      doFit();
    };
    recreate.current = createTerminal;
    createTerminal(active.current.session);
    const observer = new ResizeObserver(doFit); observer.observe(container.current);
    const frame = requestAnimationFrame(doFit);
    const unsubscribe = subscribe(event => {
      if (disposed || !sameSession(event.session, active.current.session)) return;
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
    return () => { disposed = true; recreate.current = () => {}; cancelAnimationFrame(frame); clearTimeout(resizeTimer); observer.disconnect(); unsubscribe(); releaseInput?.(); terminal?.dispose(); termRef.current = null; searchRef.current = null; fitRef.current = () => {}; };
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

  useEffect(() => {
    const terminal = termRef.current;
    if (terminal) { terminal.options.fontSize = settings.fontSize; terminal.options.scrollback = settings.scrollback; fitRef.current(); }
  }, [settings.fontSize, settings.scrollback]);
  useEffect(() => {
    if (!searchOpen) return;
    if (query) searchRef.current?.findNext(query, { incremental: true, regex: false, decorations: SEARCH_DECORATIONS });
    else { searchRef.current?.clearDecorations(); setResults({ resultIndex: -1, resultCount: 0 }); }
  }, [query, searchOpen]);
  useEffect(() => { fitRef.current(); }, [searchOpen, paste]);
  const closeSearch = () => { setSearchOpen(false); searchRef.current?.clearDecorations(); termRef.current?.focus(); };
  const find = (previous: boolean) => {
    if (!query) return;
    if (previous) searchRef.current?.findPrevious(query, { regex: false, decorations: SEARCH_DECORATIONS });
    else searchRef.current?.findNext(query, { regex: false, decorations: SEARCH_DECORATIONS });
  };
  const copySelection = async () => {
    const value = termRef.current?.getSelection();
    if (!value || clipboardLock.current) return;
    const captured = displayedSession.current;
    clipboardLock.current = true; setClipboardBusy(true);
    try {
      if (!navigator.clipboard?.writeText) throw new Error('Clipboard unavailable');
      await navigator.clipboard.writeText(value);
      if (sameSession(captured, active.current.session)) setToolMessage('选区已复制。Ctrl+C 仍发送终端中断。');
    } catch { setToolMessage('无法访问剪贴板。请使用系统复制操作；没有发送终端输入。'); }
    finally { clipboardLock.current = false; setClipboardBusy(false); }
  };
  const previewPaste = async () => {
    const captured = active.current.session;
    if (!captured || active.current.state !== 'connected' || clipboardLock.current) return;
    clipboardLock.current = true; setClipboardBusy(true); setToolMessage(null);
    try {
      if (!navigator.clipboard?.readText) throw new Error('Clipboard unavailable');
      const text = await navigator.clipboard.readText();
      if (!sameSession(captured, active.current.session) || active.current.state !== 'connected') { setToolMessage('读取期间连接已变化，剪贴板内容未发送。'); return; }
      if (terminalBytes(text).byteLength > MAX_CLIPBOARD_BYTES) { setToolMessage('剪贴板超过 16 KiB，请缩短后再粘贴。未发送任何内容。'); return; }
      if (!text) { setToolMessage('剪贴板没有文本。'); return; }
      if (unsafePasteControls(text)) { setToolMessage('剪贴板包含不可见控制字符，已阻止粘贴。请先在文本编辑器中清理。'); return; }
      setPaste({ text, session: { ...captured } });
    } catch { setToolMessage('无法读取剪贴板。请使用系统粘贴快捷键打开预览；此按钮未发送任何内容。'); }
    finally { clipboardLock.current = false; setClipboardBusy(false); }
  };
  const confirmPaste = () => {
    if (!paste || !sameSession(paste.session, active.current.session) || active.current.state !== 'connected' || terminalBytes(paste.text).byteLength > MAX_CLIPBOARD_BYTES || unsafePasteControls(paste.text)) { setPaste(null); setToolMessage('连接已变化或文本超限，未粘贴。'); return; }
    // xterm applies bracketed-paste mode when the remote shell supports it.
    termRef.current?.paste(paste.text); setPaste(null); termRef.current?.focus();
  };
  const attachSelection = () => {
    if (!sameSession(displayedSession.current, active.current.session)) return;
    const selection = captureSelection(termRef.current?.getSelection() ?? '', active.current);
    if (!selection) { setToolMessage('请先在当前已连接终端中选择输出。'); return; }
    const accepted = onAttachSelection?.(selection);
    setToolMessage(accepted ? '已加入当前聊天的附件草稿。请审阅、编辑并确认接收方后再发送。' : '无法加入此聊天。请新建与当前会话匹配的聊天，或先移除已有选区附件。');
  };

  useEffect(() => { if (historyUnavailable) setGap('之前的终端历史未恢复。若输出不再更新，请明确断开后重新连接，不要重放旧命令。'); }, [historyUnavailable]);

  const online = snapshot.state === 'connected';
  const busy = ['connecting', 'awaiting_host_key', 'authenticating', 'closing'].includes(snapshot.state);
  return <section className="terminal-pane" aria-label="SSH 终端" onKeyDownCapture={event => {
    if ((event.ctrlKey || event.metaKey) && event.shiftKey && !event.altKey) {
      if (event.key.toLowerCase() === 'f') { event.preventDefault(); event.stopPropagation(); setSearchOpen(true); requestAnimationFrame(() => searchInput.current?.focus()); }
      else if (container.current?.contains(event.target as Node) && event.key.toLowerCase() === 'c') { event.preventDefault(); event.stopPropagation(); void copySelection(); }
      else if (container.current?.contains(event.target as Node) && event.key.toLowerCase() === 'v') { event.preventDefault(); event.stopPropagation(); void previewPaste(); }
    }
  }} onPasteCapture={event => {
    if (!container.current?.contains(event.target as Node)) return;
    event.preventDefault(); event.stopPropagation();
    const text = event.clipboardData.getData('text/plain'); const target = active.current;
    if (!target.session || target.state !== 'connected') return;
    if (terminalBytes(text).byteLength > MAX_CLIPBOARD_BYTES || unsafePasteControls(text)) { setToolMessage('粘贴文本超限或含不可见控制字符，已阻止。请缩短或清理后重试。'); return; }
    if (text) setPaste({ text, session: { ...target.session } });
  }}>
    <div className="pane-toolbar"><div className="pane-tab"><Icon name="terminal" size={16} /><span>{snapshot.host ? `${snapshot.username}@${snapshot.host}` : 'SSH 终端'}</span><span className={`status-dot ${online ? 'online' : busy ? 'waiting' : ''}`} /></div><div className="toolbar-actions"><span className="protocol-tag">SSH</span>{online && <button className="terminal-interrupt" title="明确发送 Ctrl+C 到当前终端；不停止 AI 流程，也不保证远端程序被中断" onClick={() => input.current?.enqueue(terminalBytes('\x03'))}>发送 Ctrl+C</button>}{online || busy ? <button className="icon-button" aria-label="断开 SSH 连接" title="断开 SSH 连接，不保证远端程序已停止" disabled={snapshot.state === 'closing'} onClick={onDisconnect}><Icon name="disconnect" size={16} /></button> : <button className="icon-button" aria-label="新建 SSH 连接" title="新建 SSH 连接" onClick={onConnect} disabled={!available}><Icon name="plus" size={17} /></button>}</div></div>
    <div className="terminal-tools" aria-label="终端工具"><button onClick={() => { setSearchOpen(true); requestAnimationFrame(() => searchInput.current?.focus()); }} title="搜索当前终端回滚 · Ctrl / ⌘ + Shift + F">搜索</button><button disabled={!selected || clipboardBusy} onClick={() => void copySelection()}>复制选区</button><button disabled={!online || clipboardBusy} onClick={() => void previewPaste()}>粘贴…</button><button disabled={!online || !selected || !onAttachSelection} onClick={attachSelection}>选区交给 AI…</button></div>
    {searchOpen && <div className="terminal-search" role="search" aria-label="搜索终端输出" onKeyDown={event => { if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); closeSearch(); } }}><input ref={searchInput} aria-label="搜索终端文本" value={query} maxLength={256} placeholder="搜索回滚中的文本" onChange={event => setQuery(event.target.value)} onKeyDown={event => { if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); closeSearch(); } else if (event.key === 'Enter' && !event.nativeEvent.isComposing) { event.preventDefault(); find(event.shiftKey); } }} /><span role="status">{query ? results.resultCount ? `${results.resultIndex >= 0 ? results.resultIndex + 1 : '—'} / ${results.resultCount}${results.resultCount >= 1000 ? '+' : ''}` : '没有匹配' : '输入搜索词'}</span><button disabled={!query} aria-label="上一个匹配" onClick={() => find(true)}>↑</button><button disabled={!query} aria-label="下一个匹配" onClick={() => find(false)}>↓</button><button aria-label="关闭终端搜索" title="Esc" onClick={closeSearch}>关闭</button></div>}
    {paste && <section className="terminal-clipboard-preview" aria-label="确认终端粘贴"><p>粘贴到 {snapshot.username}@{snapshot.host} · 会话 {paste.session.generation}。换行可能执行命令；请核对后再确认。</p><textarea aria-label="待粘贴文本" value={paste.text} maxLength={MAX_CLIPBOARD_BYTES} onChange={event => setPaste({ ...paste, text: event.target.value })} /><button className="button secondary" onClick={() => setPaste(null)}>取消粘贴</button><button className="button primary" disabled={!online || !sameSession(paste.session, snapshot.session) || !paste.text || terminalBytes(paste.text).byteLength > MAX_CLIPBOARD_BYTES || unsafePasteControls(paste.text)} onClick={confirmPaste}>确认粘贴到终端</button></section>}
    {toolMessage && <div className="terminal-tools-status" role="status">{toolMessage}</div>}
    {gap && <div className="terminal-warning" role="alert"><Icon name="info" size={15} />{gap}</div>}
    <div className="terminal-stage"><div className={`xterm-container ${!online && !hasOutput ? 'quiet' : ''}`} ref={container} />
      {!online && !hasOutput && <div className="terminal-empty"><div className="terminal-glyph"><Icon name="terminal" size={34} /></div><div className="eyebrow">YOUR REMOTE WORKSPACE</div><h1>{busy ? SESSION_LABELS[snapshot.state] : '从一条安全连接开始'}</h1><p>{busy ? '正在建立可见的 SSH 终端。连接成功后即可手动输入。' : '熟悉的终端，专注的工作空间。\n连接你的服务器，让每一步操作清晰可见。'}</p>{!busy && <button className="button primary connect-hero" onClick={onConnect} disabled={!available}><Icon name="plus" size={17} />连接服务器</button>}<div className="empty-footnote"><Icon name="shield" size={14} /><span>{available ? '主机身份核验 · 凭据仅用于本次连接' : '当前为网页预览，SSH 需在桌面应用中使用'}</span></div></div>}
    </div>
    <div className="terminal-status"><span><span className={`status-dot ${online ? 'online' : busy ? 'waiting' : ''}`} />{SESSION_LABELS[snapshot.state]}</span><span>{online && snapshot.port ? `端口 ${snapshot.port} · ` : ''}UTF-8<span className="status-divider">|</span>{dimensions.cols} × {dimensions.rows}</span></div>
  </section>;
}
