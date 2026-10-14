import { StrictMode } from 'react';
import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AgentEvent, TerminalTaskSnapshot } from '../lib/agent';
import { actionIdentity } from '../lib/agent';
import type { SessionSnapshot } from '../lib/types';
import { chat, modelConfig } from '../test/modelFixtures';
import { action, connectedSession, startRequest, task } from '../test/agentFixtures';
const mocks = vi.hoisted(() => ({ native: vi.fn(), listen: vi.fn(), list: vi.fn(), get: vi.fn(), start: vi.fn(), ack: vi.fn(), decide: vi.fn(), cancel: vi.fn(), continue: vi.fn(), prepareSecret: vi.fn() }));
vi.mock('../lib/bridge', () => ({ desktopAvailable: mocks.native }));
vi.mock('../lib/agentBridge', () => ({ agentBridge: mocks }));
import { useTerminalAgent } from './useTerminalAgent';
let emit: (event: AgentEvent) => void;
beforeEach(() => {
  vi.resetAllMocks(); mocks.native.mockReturnValue(true);
  mocks.listen.mockImplementation(async handler => { emit = handler; return vi.fn(); });
  mocks.list.mockResolvedValue([]); mocks.get.mockResolvedValue(null); mocks.start.mockResolvedValue(task());
});
async function ready(saved: TerminalTaskSnapshot[] = []) {
  mocks.list.mockResolvedValue(saved);
  const hook = renderHook(({ session = connectedSession }: { session?: SessionSnapshot }) => useTerminalAgent(modelConfig, session), { initialProps: {} });
  await waitFor(() => expect(hook.result.current.ready).toBe(true));
  return hook;
}
describe('M3 terminal agent authority and recovery', () => {
  it('binds prepared secret input to the paused owned run, prevents duplicate mutation and rejects stale connection', async () => {
    const paused = task({ phase: 'needs_secrets' });
    const { result, rerender } = await ready([paused]);
    mocks.prepareSecret.mockResolvedValue(task({ revision: '2', phase: 'awaiting_display', action: action({ policy: 'require_approval', command_text: '[Secret input: service_password]' }) }));
    await act(async () => { await result.current.prepareSecret('chat-a', 'wrong-run', 'service_password', 'Auth'); });
    expect(mocks.prepareSecret).not.toHaveBeenCalled();
    await act(async () => { await result.current.prepareSecret('chat-a', 'run-a', 'service_password', 'Auth'); });
    expect(mocks.prepareSecret).toHaveBeenCalledExactlyOnceWith({ task_id: 'chat-a', run_id: 'run-a', field_name: 'service_password', purpose: 'Auth', prompt_confirmed: true });
    await act(async () => { await result.current.prepareSecret('chat-a', 'run-a', 'service_password', 'Auth'); });
    expect(mocks.prepareSecret).toHaveBeenCalledOnce(); expect(mocks.decide).not.toHaveBeenCalled();
    rerender({ session: { ...connectedSession, session: { session_id: 'other', generation: '2' } } });
    await act(async () => { await result.current.prepareSecret('chat-a', 'run-a', 'service_password', 'Auth'); });
    expect(mocks.prepareSecret).toHaveBeenCalledOnce();
  });

  it('subscribes before recovery, including StrictMode, without starting or acknowledging any task', async () => {
    mocks.list.mockResolvedValue([task({ phase: 'awaiting_display', action: action() })]);
    const { result } = renderHook(() => useTerminalAgent(modelConfig, connectedSession), { wrapper: StrictMode });
    await waitFor(() => expect(result.current.ready).toBe(true));
    expect(mocks.listen.mock.invocationCallOrder[0]).toBeLessThan(mocks.list.mock.invocationCallOrder[0]);
    expect(result.current.owner?.task_id).toBe('chat-a');
    expect(mocks.start).not.toHaveBeenCalled(); expect(mocks.ack).not.toHaveBeenCalled(); expect(mocks.continue).not.toHaveBeenCalled();
  });
  it('requires exact current endpoint consent, connected generation, readiness and nonempty bounded instruction', async () => {
    const { result } = await ready();
    const requests = [startRequest({ context_consent: { revision: '0', destination: modelConfig.destination! } }), startRequest({ context_consent: { revision: '1', destination: 'https://elsewhere.invalid' } }), startRequest({ session: { session_id: 'old', generation: '0' } }), startRequest({ readiness_confirmed: false as true }), startRequest({ instruction: '中'.repeat(5500) }), startRequest({ instruction: '' })];
    for (const request of requests) await act(async () => { expect(await result.current.start(chat(), request)).toBe(false); });
    expect(mocks.start).not.toHaveBeenCalled();
    await act(async () => { expect(await result.current.start(chat(), startRequest())).toBe(true); });
    expect(mocks.start).toHaveBeenCalledExactlyOnceWith(startRequest());
  });
  it('collects terminal consent separately even when text-chat consent was not given', async () => {
    const { result } = renderHook(() => useTerminalAgent({ ...modelConfig, consented: false }, connectedSession));
    await waitFor(() => expect(result.current.ready).toBe(true));
    await act(async () => { expect(await result.current.start(chat(), startRequest())).toBe(true); });
    expect(mocks.start).toHaveBeenCalledOnce();
  });
  it('prevents repeated starts and multiple writers while an IPC response is pending', async () => {
    let complete!: (next: TerminalTaskSnapshot) => void;
    mocks.start.mockImplementation(() => new Promise(resolve => { complete = resolve; }));
    const { result } = await ready();
    let starting!: Promise<boolean>;
    act(() => { starting = result.current.start(chat(), startRequest()); });
    await act(async () => { expect(await result.current.start(chat(), startRequest())).toBe(false); expect(await result.current.start(chat('chat-b'), startRequest({ chat_id: 'chat-b' }))).toBe(false); });
    await act(async () => { complete(task()); await starting; });
    expect(mocks.start).toHaveBeenCalledOnce(); expect(result.current.owner?.task_id).toBe('chat-a');
  });
  it('keeps a single owner across chat drafts and never transfers ownership on reads', async () => {
    const { result } = await ready([task()]);
    act(() => { result.current.updateDraft('chat-a', 'A'); result.current.updateDraft('chat-b', 'B'); });
    await act(async () => { await result.current.refresh(); expect(await result.current.start(chat('chat-b'), startRequest({ chat_id: 'chat-b' }))).toBe(false); });
    expect(result.current.drafts).toEqual({ 'chat-a': 'A', 'chat-b': 'B' }); expect(result.current.owner?.chat_id).toBe('chat-a');
    expect(mocks.start).not.toHaveBeenCalled(); expect(mocks.cancel).not.toHaveBeenCalled();
  });
  it('discards late old-run snapshots using monotonic bigint revisions', async () => {
    const current = task({ revision: '9007199254740994', run_id: 'new-run', assistant_text: 'current' });
    const { result } = await ready([current]);
    act(() => emit({ kind: 'task_updated', task: task({ revision: '9007199254740993', run_id: 'old-run', assistant_text: 'stale' }) }));
    expect(result.current.tasks['chat-a'].assistant_text).toBe('current');
    await act(async () => result.current.cancel('chat-a', 'old-run'));
    expect(mocks.cancel).not.toHaveBeenCalled();
  });
  it('acks immutable identity only once and never performs approval on receipt of model events', async () => {
    const prepared = task({ phase: 'awaiting_display', action: action() });
    const { result } = await ready([prepared]);
    mocks.ack.mockResolvedValue({ ...prepared, revision: '2', phase: 'dispatching', action: { ...prepared.action!, displayed: true, delivery: 'queued' } });
    expect(mocks.ack).not.toHaveBeenCalled();
    await act(async () => { await result.current.acknowledge(actionIdentity(prepared)!); await result.current.acknowledge(actionIdentity(prepared)!); });
    expect(mocks.ack).toHaveBeenCalledExactlyOnceWith(actionIdentity(prepared)); expect(mocks.decide).not.toHaveBeenCalled();
  });
  it('binds a single explicit decision to the displayed exact action and rejects stale targets', async () => {
    const waiting = task({ phase: 'waiting_approval', action: action({ displayed: true, policy: 'require_approval', approval_id: 'approval-a' }) });
    const { result } = await ready([waiting]);
    mocks.decide.mockResolvedValue({ ...waiting, revision: '2', phase: 'needs_user' });
    await act(async () => {
      await result.current.decide({ ...actionIdentity(waiting)!, action_id: 'old-action' }, 'approval-a', 'approve');
      await result.current.decide(actionIdentity(waiting)!, 'wrong-approval', 'approve');
      await result.current.decide(actionIdentity(waiting)!, 'approval-a', 'deny');
      await result.current.decide(actionIdentity(waiting)!, 'approval-a', 'approve');
    });
    expect(mocks.decide).toHaveBeenCalledExactlyOnceWith({ task_id: 'chat-a', run_id: 'run-a', action_id: 'action-a', approval_id: 'approval-a', decision: 'deny' });
  });
  it('does not reuse an old display acknowledgement after reconnecting', async () => {
    const prepared = task({ phase: 'awaiting_display', action: action() });
    const { result, rerender } = await ready([prepared]);
    rerender({ session: { ...connectedSession, session: { session_id: 'ssh-b', generation: '2' } } });
    await act(async () => result.current.acknowledge(actionIdentity(prepared)!));
    expect(mocks.ack).not.toHaveBeenCalled(); expect(mocks.start).not.toHaveBeenCalled(); expect(mocks.continue).not.toHaveBeenCalled();
  });
  it('keeps cancellation available while display acknowledgement is in flight and never sends Ctrl+C', async () => {
    const prepared = task({ phase: 'awaiting_display', action: action() });
    const { result } = await ready([prepared]);
    let finishAck!: (value: TerminalTaskSnapshot) => void;
    mocks.ack.mockImplementation(() => new Promise(resolve => { finishAck = resolve; }));
    mocks.cancel.mockResolvedValue({ ...prepared, revision: '3', phase: 'cancelled', owner: false });
    let acknowledging!: Promise<void>;
    act(() => { acknowledging = result.current.acknowledge(actionIdentity(prepared)!); });
    await act(async () => result.current.cancel('chat-a', 'run-a'));
    await act(async () => { finishAck({ ...prepared, revision: '2', phase: 'dispatching' }); await acknowledging; });
    expect(mocks.cancel).toHaveBeenCalledExactlyOnceWith({ task_id: 'chat-a', run_id: 'run-a' });
    expect(result.current.tasks['chat-a'].phase).toBe('cancelled'); expect(mocks.continue).not.toHaveBeenCalled();
  });
  it('does not repeat ambiguous starts or execute during read-only reconciliation', async () => {
    const { result } = await ready();
    mocks.start.mockRejectedValue(new Error('untrusted transport failure'));
    await act(async () => { expect(await result.current.start(chat(), startRequest())).toBe(false); });
    expect(result.current.uncertain['chat-a']).toBe('0'); expect(result.current.blocked).toBe(true);
    await act(async () => { await result.current.refresh(); expect(await result.current.start(chat(), startRequest())).toBe(false); });
    expect(mocks.start).toHaveBeenCalledOnce();
    mocks.list.mockResolvedValue([task()]);
    await act(async () => { document.dispatchEvent(new Event('visibilitychange')); });
    expect(result.current.uncertain['chat-a']).toBeUndefined(); expect(result.current.owner?.task_id).toBe('chat-a');
    expect(mocks.ack).not.toHaveBeenCalled();
  });
  it('requires explicit continuation after unknown completion and keeps historical outcome unknown', async () => {
    const unknown = task({ phase: 'needs_user', terminal_ready: false, remote_outcome: 'unknown', action: action({ displayed: true, delivery: 'write_accepted' }) });
    const { result } = await ready([unknown]);
    expect(mocks.continue).not.toHaveBeenCalled();
    mocks.continue.mockResolvedValue({ ...unknown, revision: '2', phase: 'preparing', round: 2, terminal_ready: true });
    await act(async () => result.current.continueTask('chat-a', 'run-a'));
    expect(mocks.continue).toHaveBeenCalledExactlyOnceWith({ task_id: 'chat-a', run_id: 'run-a', readiness_confirmed: true });
    expect(result.current.tasks['chat-a'].remote_outcome).toBe('unknown');
  });
  it('blocks a new run after cancelled unknown output until explicit readiness is recorded', async () => {
    const cancelled = task({ owner: false, phase: 'cancelled', terminal_ready: false, remote_outcome: 'unknown' });
    const { result } = await ready([cancelled]);
    expect(result.current.blocked).toBe(true);
    mocks.continue.mockResolvedValue({ ...cancelled, revision: '2', terminal_ready: true });
    await act(async () => result.current.continueTask('chat-a', 'run-a'));
    expect(result.current.blocked).toBe(false); expect(result.current.tasks['chat-a'].phase).toBe('cancelled');
    expect(result.current.tasks['chat-a'].remote_outcome).toBe('unknown'); expect(mocks.start).not.toHaveBeenCalled();
  });
  it('recovers a definitive start preflight rejection without automatically retrying or stranding the UI', async () => {
    const { result } = await ready();
    mocks.start.mockRejectedValueOnce({ code: 'MODEL_CONFIG_CHANGED', safe_message: 'Review the changed model destination.', retry: 'after_user_action' });
    await act(async () => { expect(await result.current.start(chat(), startRequest())).toBe(false); });
    expect(result.current.uncertain['chat-a']).toBeUndefined(); expect(result.current.blocked).toBe(false);
    expect(mocks.start).toHaveBeenCalledOnce();
    await act(async () => { expect(await result.current.start(chat(), startRequest())).toBe(true); });
    expect(mocks.start).toHaveBeenCalledTimes(2);
  });
  it('does not start execution in a chat with an active text-generation run', async () => {
    const { result } = await ready();
    await act(async () => { expect(await result.current.start({ ...chat(), active_run_id: 'text-run' }, startRequest())).toBe(false); });
    expect(mocks.start).not.toHaveBeenCalled();
  });
  it('never invokes execution IPC in browser preview and cleans subscriptions without cancellation', async () => {
    mocks.native.mockReturnValue(false);
    const { result, unmount } = renderHook(() => useTerminalAgent(modelConfig, connectedSession));
    act(() => result.current.updateDraft('chat-a', 'preview'));
    await act(async () => expect(await result.current.start(chat(), startRequest())).toBe(false));
    unmount(); expect(mocks.listen).not.toHaveBeenCalled(); expect(mocks.start).not.toHaveBeenCalled(); expect(mocks.cancel).not.toHaveBeenCalled();
  });
});
