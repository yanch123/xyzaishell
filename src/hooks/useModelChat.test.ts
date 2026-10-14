import { StrictMode } from 'react';
import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ChatSnapshot, ModelEvent } from '../lib/model';
import { chat, modelConfig, stream } from '../test/modelFixtures';

const mocks = vi.hoisted(() => ({ native: vi.fn(), listen: vi.fn(), config: vi.fn(), list: vi.fn(), create: vi.fn(), get: vi.fn(), send: vi.fn(), cancel: vi.fn(), reload: vi.fn(), confirm: vi.fn(), probe: vi.fn(), save: vi.fn(), testSaved: vi.fn(), cancelTest: vi.fn() }));
vi.mock('../lib/bridge', () => ({ desktopAvailable: mocks.native }));
vi.mock('../lib/modelBridge', () => ({ modelBridge: mocks }));
import { useModelChat } from './useModelChat';
let emit: (event: ModelEvent) => void;
beforeEach(() => {
  vi.resetAllMocks();
  mocks.native.mockReturnValue(true);
  mocks.listen.mockImplementation(async handler => { emit = handler; return vi.fn(); });
  mocks.config.mockResolvedValue({ ...modelConfig });
  mocks.list.mockResolvedValue([chat()]);
  mocks.create.mockResolvedValue(chat('chat-b'));
  mocks.get.mockImplementation(async (id: string) => chat(id));
  mocks.send.mockImplementation(async (id: string) => stream(id));
  mocks.cancel.mockImplementation(async (id: string) => ({ ...stream(id, '4', 'partial'), active_run_id: null, messages: stream(id, '4', 'partial').messages.map(message => message.role === 'assistant' ? { ...message, state: 'cancelled' } : message) }));
  mocks.confirm.mockResolvedValue({ ...modelConfig, revision: '2', consented: true });
  mocks.reload.mockResolvedValue({ ...modelConfig, revision: '3', consented: false });
  mocks.probe.mockResolvedValue(stream('probe'));
});
async function ready() {
  const hook = renderHook(useModelChat);
  await waitFor(() => expect(hook.result.current.ready).toBe(true));
  return hook;
}

describe('M2 canonical chat lifecycle', () => {
  it('subscribes before initial reads, and sends nothing automatically', async () => {
    const { result } = await ready();
    expect(mocks.listen.mock.invocationCallOrder[0]).toBeLessThan(mocks.list.mock.invocationCallOrder[0]);
    expect(result.current.current.chat_id).toBe('chat-a');
    expect(mocks.send).not.toHaveBeenCalled(); expect(mocks.probe).not.toHaveBeenCalled();
  });
  it('creates one initial chat under StrictMode and never replays a request', async () => {
    mocks.list.mockResolvedValue([]);
    const { result } = renderHook(useModelChat, { wrapper: StrictMode });
    await waitFor(() => expect(result.current.ready).toBe(true));
    expect(mocks.create).toHaveBeenCalledOnce(); expect(mocks.send).not.toHaveBeenCalled();
  });
  it('keeps preview drafts in memory without invoking the native model bridge', async () => {
    mocks.native.mockReturnValue(false);
    const { result } = renderHook(useModelChat);
    const id = result.current.current.chat_id;
    act(() => result.current.updateDraft(id, 'draft'));
    await act(async () => { expect(await result.current.send(id)).toBe(false); await result.current.createChat(); });
    expect(result.current.chats).toHaveLength(2);
    act(() => result.current.selectChat(id));
    expect(result.current.drafts[id]).toBe('draft');
    expect(mocks.listen).not.toHaveBeenCalled(); expect(mocks.send).not.toHaveBeenCalled(); expect(mocks.create).not.toHaveBeenCalled();
  });
  it('does not replace a streaming snapshot with stale send replies or events', async () => {
    let reply!: (value: ChatSnapshot) => void;
    mocks.send.mockImplementation(() => new Promise(resolve => { reply = resolve; }));
    const { result } = await ready();
    act(() => result.current.updateDraft('chat-a', 'hello'));
    let sending!: Promise<boolean>;
    act(() => { sending = result.current.send('chat-a'); });
    act(() => emit({ kind: 'chat_updated', chat: stream('chat-a', '9007199254740994', 'streamed') }));
    await act(async () => { reply(stream('chat-a', '2')); await sending; });
    act(() => emit({ kind: 'chat_updated', chat: stream('chat-a', '9007199254740993', 'stale') }));
    expect(result.current.current.messages[1].text).toBe('streamed');
    expect(result.current.drafts['chat-a']).toBe('');
    const complete = { ...stream('chat-a', '9007199254740995', 'finished'), active_run_id: null };
    complete.messages[1].state = 'complete';
    act(() => emit({ kind: 'chat_updated', chat: complete }));
    expect(result.current.runningCount).toBe(0); expect(result.current.current.messages[1].state).toBe('complete');
  });
  it('keeps two chats and drafts isolated while switching, with no cancellation', async () => {
    mocks.list.mockResolvedValue([chat(), chat('chat-b')]);
    const { result } = await ready();
    act(() => { result.current.updateDraft('chat-a', 'message A'); result.current.updateDraft('chat-b', 'message B'); });
    await act(async () => { await result.current.send('chat-a'); result.current.selectChat('chat-b'); });
    expect(result.current.current.chat_id).toBe('chat-b'); expect(result.current.drafts['chat-b']).toBe('message B');
    act(() => emit({ kind: 'chat_updated', chat: stream('chat-a', '3', 'answer A') }));
    expect(result.current.current.messages).toHaveLength(0);
    await act(async () => { await result.current.send('chat-b'); });
    expect(mocks.send.mock.calls).toEqual([['chat-a', 'message A'], ['chat-b', 'message B']]);
    expect(result.current.runningCount).toBe(2); expect(mocks.cancel).not.toHaveBeenCalled();
  });
  it('cancels the exact chat and run, retains partial text, ignores late events', async () => {
    mocks.list.mockResolvedValue([stream('chat-a', '3', 'partial')]);
    const { result } = await ready();
    await act(async () => { await result.current.cancel('chat-a'); });
    expect(mocks.cancel).toHaveBeenCalledWith('chat-a', 'run-chat-a');
    expect(result.current.current.messages[1].state).toBe('cancelled');
    act(() => emit({ kind: 'chat_updated', chat: stream('chat-a', '3', 'late') }));
    expect(result.current.current.messages[1].text).toBe('partial'); expect(result.current.current.active_run_id).toBeNull();
  });
  it('shows safe errors, retains failed drafts and reconciles without automatic retry', async () => {
    mocks.send.mockRejectedValue(new Error('api_key=SECRET'));
    const { result } = await ready();
    act(() => result.current.updateDraft('chat-a', 'keep draft'));
    await act(async () => { expect(await result.current.send('chat-a')).toBe(false); });
    expect(result.current.errors['chat-a']).not.toContain('SECRET'); expect(result.current.drafts['chat-a']).toBe('keep draft');
    expect(mocks.send).toHaveBeenCalledOnce(); expect(mocks.get).toHaveBeenCalledWith('chat-a');
  });
  it('guards repeated clicks and preserves a new draft edited before IPC resolves', async () => {
    let reply!: (value: ChatSnapshot) => void;
    mocks.send.mockImplementation(() => new Promise(resolve => { reply = resolve; }));
    const { result } = await ready();
    act(() => result.current.updateDraft('chat-a', 'first'));
    let sending!: Promise<boolean>;
    act(() => { sending = result.current.send('chat-a'); void result.current.send('chat-a'); result.current.updateDraft('chat-a', 'next draft'); });
    await act(async () => { reply(stream()); await sending; });
    expect(mocks.send).toHaveBeenCalledOnce(); expect(result.current.drafts['chat-a']).toBe('next draft');
  });
  it('enforces the UTF-8 input limit rather than JavaScript character count', async () => {
    const { result } = await ready();
    act(() => result.current.updateDraft('chat-a', '中'.repeat(5500)));
    await act(async () => { expect(await result.current.send('chat-a')).toBe(false); });
    expect(mocks.send).not.toHaveBeenCalled();
  });
  it('requires destination consent, binds approval to revision and invalidates it on reload', async () => {
    mocks.config.mockResolvedValue({ ...modelConfig, consented: false });
    const { result } = await ready();
    act(() => result.current.updateDraft('chat-a', 'hello'));
    await act(async () => { expect(await result.current.send('chat-a')).toBe(false); });
    expect(mocks.send).not.toHaveBeenCalled();
    await act(async () => { expect(await result.current.confirmDestination(result.current.config)).toBe(true); });
    expect(mocks.confirm).toHaveBeenCalledWith('1', modelConfig.destination);
    await act(async () => { await result.current.reloadConfig(); });
    act(() => emit({ kind: 'config_updated', config: { ...modelConfig, revision: '2', consented: true } }));
    expect(result.current.config.revision).toBe('3'); expect(result.current.config.consented).toBe(false);
  });
  it('limits global streams without cancelling or moving ownership', async () => {
    mocks.list.mockResolvedValue([stream('a'), stream('b'), stream('c'), stream('d'), chat('e')]);
    const { result } = await ready();
    act(() => result.current.updateDraft('e', 'fifth'));
    await act(async () => { expect(await result.current.send('e')).toBe(false); result.current.selectChat('e'); });
    expect(result.current.runningCount).toBe(4); expect(mocks.send).not.toHaveBeenCalled(); expect(mocks.cancel).not.toHaveBeenCalled();
  });
  it('runs a compatibility probe only after explicit invocation and shows its own chat', async () => {
    const { result } = await ready();
    expect(mocks.probe).not.toHaveBeenCalled();
    await act(async () => { expect(await result.current.probe()).toBe(true); });
    expect(mocks.probe).toHaveBeenCalledOnce(); expect(result.current.current.chat_id).toBe('probe');
  });
  it('removes the event subscription on unmount and does not cancel backend chats', async () => {
    const unlisten = vi.fn(); mocks.listen.mockResolvedValue(unlisten);
    const { unmount } = await ready();
    unmount(); expect(unlisten).toHaveBeenCalledOnce(); expect(mocks.cancel).not.toHaveBeenCalled();
  });
});

describe('read-only recovery', () => {
  it('reconciles a dropped final event on visibility without restarting or cancelling the run', async () => {
    mocks.list.mockResolvedValue([stream('chat-a', '2', 'partial')]);
    const final = { ...stream('chat-a', '3', 'complete'), active_run_id: null }; final.messages[1].state = 'complete';
    mocks.get.mockResolvedValue(final);
    const { result } = await ready();
    expect(result.current.runningCount).toBe(1);
    await act(async () => { document.dispatchEvent(new Event('visibilitychange')); });
    expect(result.current.runningCount).toBe(0); expect(result.current.current.messages[1].text).toBe('complete');
    expect(mocks.send).not.toHaveBeenCalled(); expect(mocks.cancel).not.toHaveBeenCalled();
  });
  it('lets the user recover a failed initialization without resending messages', async () => {
    mocks.listen.mockRejectedValueOnce(new Error('private backend failure'));
    const { result } = renderHook(useModelChat);
    await waitFor(() => expect(result.current.error).toBeTruthy());
    expect(result.current.ready).toBe(false);
    act(() => result.current.retryLoad());
    await waitFor(() => expect(result.current.ready).toBe(true));
    expect(result.current.error).toBeNull(); expect(mocks.send).not.toHaveBeenCalled();
  });
});

describe('saved settings and immutable chat provenance', () => {
  const target = { state: 'connected' as const, host: 'first.example', port: 22, username: 'user', session: { session_id: 'first-session', generation: '1' } };
  it('creates a blank bound conversation on target changes, but preserves same-target reconnect text', async () => {
    mocks.create.mockImplementation(async (_title, source) => ({ ...chat(source.session_id), source: { ...source, host: source.session_id === 'second-session' ? 'second.example' : 'first.example', port: 22, username: 'user' } }));
    const { result, rerender } = renderHook((props: { target: typeof target | undefined }) => useModelChat(props.target), { initialProps: { target: undefined as typeof target | undefined } });
    await waitFor(() => expect(result.current.ready).toBe(true));
    rerender({ target }); await waitFor(() => expect(result.current.current.chat_id).toBe('first-session'));
    expect(mocks.create).toHaveBeenCalledExactlyOnceWith('新任务', target.session);
    rerender({ target: { ...target, session: { session_id: 'reconnected-session', generation: '2' } } });
    await act(async () => {}); expect(mocks.create).toHaveBeenCalledOnce();
    rerender({ target: { ...target, host: 'second.example', session: { session_id: 'second-session', generation: '3' } } });
    await waitFor(() => expect(result.current.current.chat_id).toBe('second-session'));
    expect(mocks.create).toHaveBeenCalledTimes(2); expect(result.current.current.messages).toHaveLength(0);
    expect(mocks.send).not.toHaveBeenCalled(); expect(mocks.cancel).not.toHaveBeenCalled();
  });
  it('reconciles an existing live target chat on first load without creating a duplicate', async () => {
    const source = { ...target.session, host: target.host, port: target.port, username: target.username };
    mocks.list.mockResolvedValue([{ ...chat('already-bound'), source }]);
    const { result } = renderHook(() => useModelChat(target));
    await waitFor(() => expect(result.current.ready).toBe(true));
    expect(result.current.current.chat_id).toBe('already-bound'); expect(mocks.create).not.toHaveBeenCalled();
  });
  it('never transmits restored or different-target context', async () => {
    mocks.list.mockResolvedValue([{ ...chat('restored'), restored_read_only: true }, { ...chat('other-target'), source: { ...target.session, host: 'other.example', port: 22, username: 'user' } }]);
    const { result } = await ready();
    act(() => { result.current.updateDraft('restored', 'no replay'); result.current.updateDraft('other-target', 'wrong server'); });
    await act(async () => { expect(await result.current.send('restored')).toBe(false); expect(await result.current.send('other-target')).toBe(false); });
    expect(mocks.send).not.toHaveBeenCalled();
  });
  it('fences a late first-target creation after the connected target changed', async () => {
    let first!: (value: ChatSnapshot) => void;
    mocks.create.mockImplementationOnce(() => new Promise(resolve => { first = resolve; })).mockResolvedValueOnce(chat('second-new'));
    const { result, rerender } = renderHook((props: { target: typeof target }) => useModelChat(props.target), { initialProps: { target } });
    await waitFor(() => expect(mocks.create).toHaveBeenCalledOnce());
    rerender({ target: { ...target, host: 'second.example', session: { session_id: 'second-session', generation: '2' } } });
    await act(async () => { first(chat('old-target-late')); });
    await waitFor(() => expect(result.current.current.chat_id).toBe('second-new'));
    expect(result.current.chats.some(value => value.chat_id === 'old-target-late')).toBe(true);
    expect(mocks.send).not.toHaveBeenCalled();
  });
  it('blocks repeat saves and tests after an unknown commit until explicit reload', async () => {
    mocks.save.mockRejectedValue(new Error('synthetic-private-provider-diagnostic'));
    const { result } = await ready();
    const request = { expected_file_revision: 'saved-file-1', base_url: 'https://example.com/v1', model: 'new', key_update: { kind: 'keep' as const }, reuse_key_for_destination: null };
    await act(async () => { expect(await result.current.saveConfig(request)).toBeNull(); });
    expect(result.current.configWriteBlocked).toBe(true); expect(result.current.error).not.toContain('synthetic');
    await act(async () => { expect(await result.current.saveConfig(request)).toBeNull(); expect(await result.current.testSaved(result.current.config)).toBeNull(); });
    expect(mocks.save).toHaveBeenCalledOnce(); expect(mocks.testSaved).not.toHaveBeenCalled();
    await act(async () => { await result.current.reloadConfig(); }); expect(result.current.configWriteBlocked).toBe(false);
  });
  it('tests only an explicit saved version without creating or selecting a diagnostic chat', async () => {
    mocks.testSaved.mockImplementation(async request => ({ ...request, success: true, error: null }));
    const { result } = await ready();
    await act(async () => { await result.current.testSaved(result.current.config); });
    expect(mocks.testSaved).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ file_revision: modelConfig.file_revision, destination: modelConfig.destination, model: modelConfig.model, consent: true }));
    expect(result.current.current.chat_id).toBe('chat-a'); expect(result.current.configTest?.success).toBe(true);
    expect(mocks.probe).not.toHaveBeenCalled(); expect(mocks.create).not.toHaveBeenCalled(); expect(mocks.send).not.toHaveBeenCalled();
  });
});
