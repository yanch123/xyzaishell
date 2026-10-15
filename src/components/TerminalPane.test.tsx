import { act, fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { TerminalOutput } from '../lib/types';
type FakeTerminal = { selection: string; selectionChanged?: () => void; keyHandler?: (event: KeyboardEvent) => boolean; paste: (text: string) => void; disposed: boolean; options: { fontSize?: number; scrollback?: number; disableStdin?: boolean; linkHandler?: { activate: () => void; allowNonHttpProtocols: boolean } }; writes: { bytes: Uint8Array; done: () => void }[]; onDataCallback?: (value: string) => void };
const mocks = vi.hoisted(() => ({ instances: [] as FakeTerminal[], write: vi.fn().mockResolvedValue(undefined), ack: vi.fn().mockResolvedValue(undefined), resize: vi.fn().mockResolvedValue(undefined), next: vi.fn().mockReturnValue(true), previous: vi.fn().mockReturnValue(true), clear: vi.fn(), searchChanged: undefined as undefined | ((value: { resultIndex: number; resultCount: number }) => void) }));
vi.mock('@xterm/xterm', () => ({ Terminal: class {
  options: { fontSize?: number; scrollback?: number; disableStdin?: boolean; linkHandler?: { activate: () => void; allowNonHttpProtocols: boolean } }; disposed = false; writes: {bytes:Uint8Array;done:()=>void}[] = []; cols = 80; rows = 24;
  selection = ''; selectionChanged?: () => void; keyHandler?: (event: KeyboardEvent) => boolean;
  onDataCallback?: (value: string) => void;
  constructor(options: {disableStdin?:boolean}) { this.options = options; mocks.instances.push(this); }
  loadAddon() {} open(element: HTMLElement) { const textarea = document.createElement('textarea'); textarea.setAttribute('aria-label', 'terminal input'); element.append(textarea); } focus() {}
  onSelectionChange(callback: () => void) { this.selectionChanged = callback; return { dispose() {} }; }
  hasSelection() { return !!this.selection; } getSelection() { return this.selection; }
  attachCustomKeyEventHandler(callback: (event: KeyboardEvent) => boolean) { this.keyHandler = callback; }
  paste(text: string) { this.onDataCallback?.(text); }
  onData(callback: (value:string)=>void) { this.onDataCallback = callback; return { dispose() {} }; }
  onBinary() { return { dispose() {} }; }
  write(bytes: Uint8Array, done:()=>void) { this.writes.push({bytes,done}); }
  dispose() { this.disposed = true; }
} }));
vi.mock('@xterm/addon-search', () => ({ SearchAddon: class { findNext = mocks.next; findPrevious = mocks.previous; clearDecorations = mocks.clear; onDidChangeResults(callback: typeof mocks.searchChanged) { mocks.searchChanged = callback; return { dispose() {} }; } } }));
vi.mock('@xterm/addon-fit', () => ({ FitAddon: class { fit() {} } }));
vi.mock('../lib/bridge', () => ({ sshBridge: { write:mocks.write, ackOutput:mocks.ack, resize:mocks.resize } }));
import { TerminalPane } from './TerminalPane';
vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
const a = {session_id:'host-a',generation:'1'};
const b = {session_id:'host-b',generation:'2'};
beforeEach(() => { localStorage.clear(); mocks.next.mockClear(); mocks.previous.mockClear(); mocks.clear.mockClear(); mocks.write.mockClear(); mocks.ack.mockClear(); mocks.resize.mockClear(); });
describe('terminal generation isolation', () => {
  it('disposes queued old-host output and fences late response/ACK callbacks across reconnect', async () => {
    let output!: (event:TerminalOutput) => void;
    const subscribe = (fn:(event:TerminalOutput)=>void) => { output = fn; return () => {}; };
    const common = {available:true,onConnect:vi.fn(),onDisconnect:vi.fn(),subscribe,reportError:vi.fn(),sizeRef:{current:{cols:80,rows:24}}};
    const view = render(<TerminalPane {...common} snapshot={{session:a,state:'connected',revision:'1'}} />);
    const old = mocks.instances.at(-1)!;
    const open = vi.spyOn(window, 'open');
    old.options.linkHandler?.activate();
    expect(open).not.toHaveBeenCalled();
    expect(old.options.linkHandler?.allowNonHttpProtocols).toBe(false);
    act(() => output({type:'terminal.output',session:a,seq:'1',data_base64:btoa('OLD_HOST_OUTPUT\x1b[6n'),stream:'stdout'}));
    expect(old.writes).toHaveLength(1);
    view.rerender(<TerminalPane {...common} snapshot={{session:b,state:'connected',revision:'2'}} />);
    const current = mocks.instances.at(-1)!;
    expect(old.disposed).toBe(true); expect(current).not.toBe(old); expect(current.writes).toHaveLength(0);
    // Simulate a queued parser callback that survives disposal. It must not write
    // a reply into the new server or acknowledge against its output window.
    await act(async () => { old.onDataCallback?.('OLD_QUERY_RESPONSE'); old.writes[0].done(); });
    expect(mocks.write).not.toHaveBeenCalled(); expect(mocks.ack).not.toHaveBeenCalled();
    act(() => output({type:'terminal.output',session:b,seq:'1',data_base64:btoa('NEW_HOST_OUTPUT'),stream:'stdout'}));
    expect(current.writes).toHaveLength(1);
    expect(new TextDecoder().decode(current.writes[0].bytes)).toBe('NEW_HOST_OUTPUT');
    await act(async () => current.writes[0].done());
    expect(mocks.ack).toHaveBeenCalledExactlyOnceWith(b,'1');
    await act(async () => current.onDataCallback?.('new input'));
    expect(mocks.write).toHaveBeenCalledExactlyOnceWith(b,btoa('new input'));
  });
});


describe('explicit manual interrupt', () => {
  it('sends Ctrl+C only on its separate user button and leaves manual input available', async () => {
    const common = { available: true, onConnect: vi.fn(), onDisconnect: vi.fn(), subscribe: () => () => {}, reportError: vi.fn(), sizeRef: { current: { cols: 80, rows: 24 } } };
    render(<TerminalPane {...common} snapshot={{ session: a, state: 'connected' }} />);
    const terminal = mocks.instances.at(-1)!;
    expect(terminal.options.disableStdin).toBe(false); expect(mocks.write).not.toHaveBeenCalled();
    await act(async () => fireEvent.click(screen.getByRole('button', { name: '发送 Ctrl+C' })));
    expect(mocks.write).toHaveBeenCalledExactlyOnceWith(a, btoa('\x03'));
    await act(async () => terminal.onDataCallback?.('pwd\r'));
    expect(mocks.write).toHaveBeenLastCalledWith(a, btoa('pwd\r'));
  });
});

const liveTarget = { session: a, state: 'connected' as const, host: 'host-a.example', port: 22, username: 'tester' };
const props = () => ({ available: true, onConnect: vi.fn(), onDisconnect: vi.fn(), subscribe: () => () => {}, reportError: vi.fn(), sizeRef: { current: { cols: 80, rows: 24 } } });
describe('terminal search and explicit clipboard tools', () => {
  it('opens with Ctrl+Shift+F, searches locally and navigates/counts/closes without shell writes', () => {
    render(<TerminalPane {...props()} snapshot={liveTarget} />);
    const terminal = mocks.instances.at(-1)!;
    expect(terminal.keyHandler?.(new KeyboardEvent('keydown', { key: 'f', ctrlKey: true }))).toBe(true);
    expect(terminal.keyHandler?.(new KeyboardEvent('keydown', { key: 'c', ctrlKey: true }))).toBe(true);
    fireEvent.keyDown(screen.getByLabelText('terminal input'), { key: 'F', ctrlKey: true, shiftKey: true });
    fireEvent.change(screen.getByRole('textbox', { name: '搜索终端文本' }), { target: { value: '[literal]' } });
    expect(mocks.next).toHaveBeenLastCalledWith('[literal]', expect.objectContaining({ regex: false, incremental: true }));
    act(() => mocks.searchChanged?.({ resultIndex: 1, resultCount: 3 })); expect(screen.getByText('2 / 3')).toBeDefined();
    fireEvent.click(screen.getByRole('button', { name: '上一个匹配' })); expect(mocks.previous).toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: '下一个匹配' })); expect(mocks.next).toHaveBeenCalledTimes(2);
    fireEvent.keyDown(screen.getByRole('textbox', { name: '搜索终端文本' }), { key: 'Escape' });
    expect(screen.queryByRole('search')).toBeNull(); expect(mocks.clear).toHaveBeenCalled(); expect(mocks.write).not.toHaveBeenCalled();
  });
  it('captures selected output only after its explicit attachment action', () => {
    const attach = vi.fn().mockReturnValue(true);
    render(<TerminalPane {...props()} snapshot={liveTarget} onAttachSelection={attach} />);
    const terminal = mocks.instances.at(-1)!;
    act(() => { terminal.selection = 'remote output'; terminal.selectionChanged?.(); });
    expect(attach).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: '选区交给 AI…' }));
    expect(attach).toHaveBeenCalledWith(expect.objectContaining({ text: 'remote output', source: { ...a, host: liveTarget.host, port: 22, username: 'tester' } }));
    expect(mocks.write).not.toHaveBeenCalled(); expect(screen.getByText(/已加入当前聊天/)).toBeDefined();
  });
  it('requires a paste preview and adds no Enter; rejects invisible controls', async () => {
    const readText = vi.fn().mockResolvedValue('echo one\necho two');
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { readText, writeText: vi.fn() } });
    render(<TerminalPane {...props()} snapshot={liveTarget} />);
    await act(async () => fireEvent.click(screen.getByRole('button', { name: '粘贴…' })));
    expect(mocks.write).not.toHaveBeenCalled(); expect((screen.getByRole('textbox', { name: '待粘贴文本' }) as HTMLTextAreaElement).value).toBe('echo one\necho two');
    await act(async () => fireEvent.click(screen.getByRole('button', { name: '确认粘贴到终端' })));
    expect(mocks.write).toHaveBeenCalledExactlyOnceWith(a, btoa('echo one\necho two'));
    readText.mockResolvedValue('\x1b[200~danger');
    await act(async () => fireEvent.click(screen.getByRole('button', { name: '粘贴…' })));
    expect(screen.getByText(/包含不可见控制字符/)).toBeDefined(); expect(mocks.write).toHaveBeenCalledOnce();
  });
  it('rejects clipboard reads that complete after the connection changes', async () => {
    let complete!: (text: string) => void;
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { readText: () => new Promise<string>(resolve => { complete = resolve; }) } });
    const common = props(); const view = render(<TerminalPane {...common} snapshot={liveTarget} />);
    fireEvent.click(screen.getByRole('button', { name: '粘贴…' }));
    view.rerender(<TerminalPane {...common} snapshot={{ ...liveTarget, session: b, host: 'other.example' }} />);
    await act(async () => complete('must not cross hosts'));
    expect(screen.queryByRole('textbox', { name: '待粘贴文本' })).toBeNull(); expect(mocks.write).not.toHaveBeenCalled();
  });
  it('intercepts browser paste for preview and blocks a stale confirmation after reconnect', () => {
    const common = props(); const view = render(<TerminalPane {...common} snapshot={liveTarget} />);
    fireEvent.paste(screen.getByLabelText('terminal input'), { clipboardData: { getData: () => 'pwd\n' } });
    expect(screen.getByRole('textbox', { name: '待粘贴文本' })).toBeDefined(); expect(mocks.write).not.toHaveBeenCalled();
    view.rerender(<TerminalPane {...common} snapshot={{ ...liveTarget, session: b }} />);
    expect(screen.queryByRole('button', { name: '确认粘贴到终端' })).toBeNull(); expect(mocks.write).not.toHaveBeenCalled();
  });
  it('applies saved settings live without discarding the visible terminal generation', () => {
    render(<TerminalPane {...props()} snapshot={liveTarget} />);
    const terminal = mocks.instances.at(-1)!;
    act(() => window.dispatchEvent(new CustomEvent('xyzaishell:terminal-settings', { detail: { fontSize: 22, scrollback: 1000 } })));
    expect(mocks.instances.at(-1)).toBe(terminal); expect(terminal.disposed).toBe(false);
    expect(terminal.options.fontSize).toBe(22); expect(terminal.options.scrollback).toBe(1000);
  });
});
