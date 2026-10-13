import { act, render } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { TerminalOutput } from '../lib/types';
type FakeTerminal = { disposed: boolean; options: { disableStdin?: boolean; linkHandler?: { activate: () => void; allowNonHttpProtocols: boolean } }; writes: { bytes: Uint8Array; done: () => void }[]; onDataCallback?: (value: string) => void };
const mocks = vi.hoisted(() => ({ instances: [] as FakeTerminal[], write: vi.fn().mockResolvedValue(undefined), ack: vi.fn().mockResolvedValue(undefined), resize: vi.fn().mockResolvedValue(undefined) }));
vi.mock('@xterm/xterm', () => ({ Terminal: class {
  options: { disableStdin?: boolean; linkHandler?: { activate: () => void; allowNonHttpProtocols: boolean } }; disposed = false; writes: {bytes:Uint8Array;done:()=>void}[] = []; cols = 80; rows = 24;
  onDataCallback?: (value: string) => void;
  constructor(options: {disableStdin?:boolean}) { this.options = options; mocks.instances.push(this); }
  loadAddon() {} open() {} focus() {}
  onData(callback: (value:string)=>void) { this.onDataCallback = callback; return { dispose() {} }; }
  onBinary() { return { dispose() {} }; }
  write(bytes: Uint8Array, done:()=>void) { this.writes.push({bytes,done}); }
  dispose() { this.disposed = true; }
} }));
vi.mock('@xterm/addon-fit', () => ({ FitAddon: class { fit() {} } }));
vi.mock('../lib/bridge', () => ({ sshBridge: { write:mocks.write, ackOutput:mocks.ack, resize:mocks.resize } }));
import { TerminalPane } from './TerminalPane';
vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
const a = {session_id:'host-a',generation:'1'};
const b = {session_id:'host-b',generation:'2'};
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
