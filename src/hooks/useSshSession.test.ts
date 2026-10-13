import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SessionSnapshot, SshEvent } from '../lib/types';
const mocks = vi.hoisted(() => ({ native: vi.fn(), listen: vi.fn(), snapshot: vi.fn(), connect: vi.fn(), disconnect: vi.fn(), confirm: vi.fn() }));
vi.mock('../lib/bridge', () => ({ desktopAvailable: mocks.native, sshBridge: { listen: mocks.listen, snapshot: mocks.snapshot, connect: mocks.connect, disconnect: mocks.disconnect, confirmHostKey: mocks.confirm } }));
import { useSshSession } from './useSshSession';
let emit: (event: SshEvent) => void;
const session = { session_id: 's1', generation: '1' };
beforeEach(() => {
  mocks.native.mockReturnValue(true);
  mocks.listen.mockImplementation(async handler => { emit = handler; return vi.fn(); });
  mocks.snapshot.mockResolvedValue({session:null,state:'disconnected',revision:'0'});
  mocks.connect.mockResolvedValue({session,state:'connecting',revision:'1'});
});
describe('SSH lifecycle event ordering', () => {
  it('subscribes before snapshot and never lets a stale snapshot replace a newer event', async () => {
    let reply!: (value: SessionSnapshot) => void;
    mocks.snapshot.mockImplementation(() => new Promise(resolve => { reply = resolve; }));
    const {result} = renderHook(useSshSession);
    await waitFor(() => expect(mocks.snapshot).toHaveBeenCalledOnce());
    act(() => emit({type:'session.state',snapshot:{session,state:'connected',revision:'3'}}));
    await act(async () => reply({session:null,state:'disconnected',revision:'0'}));
    expect(result.current.snapshot.state).toBe('connected'); expect(result.current.ready).toBe(true);
  });
  it('ignores old-generation output and forwards only the current terminal stream', async () => {
    const {result} = renderHook(useSshSession); await waitFor(() => expect(result.current.ready).toBe(true));
    const output = vi.fn(); result.current.subscribeOutput(output);
    act(() => emit({type:'session.state',snapshot:{session,state:'connected',revision:'1'}}));
    act(() => emit({type:'terminal.output',session:{...session,generation:'0'},seq:'1',data_base64:'YQ==',stream:'stdout'}));
    expect(output).not.toHaveBeenCalled();
    act(() => emit({type:'terminal.output',session,seq:'1',data_base64:'YQ==',stream:'stdout'}));
    expect(output).toHaveBeenCalledOnce();
  });
  it('does not create a simulated session in browser preview', async () => {
    mocks.native.mockReturnValue(false);
    const {result} = renderHook(useSshSession);
    expect(result.current.native).toBe(false); expect(result.current.ready).toBe(false);
    expect(mocks.listen).not.toHaveBeenCalled(); expect(mocks.snapshot).not.toHaveBeenCalled();
  });
  it('does not retry failed connection requests or display raw error details', async () => {
    mocks.connect.mockRejectedValue(new Error('password=TEST_SECRET'));
    const {result} = renderHook(useSshSession); await waitFor(() => expect(result.current.ready).toBe(true));
    await act(async () => { expect(await result.current.connect({host:'test',port:22,username:'tester',session_only:true,size:{cols:80,rows:24},auth:{kind:'password',password:'TEST_SECRET'}})).toBe(false); });
    expect(mocks.connect).toHaveBeenCalledOnce(); expect(result.current.error).not.toContain('TEST_SECRET');
  });
});
