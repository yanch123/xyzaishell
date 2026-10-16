import { beforeEach, expect, it, vi } from 'vitest';
const mocks = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: mocks.listen }));
import { contextBridge } from './contextBridge';
beforeEach(() => vi.resetAllMocks());
it('keeps preview local and sends only identity/revision/mode/idempotency to compaction', async () => {
  await contextBridge.usage('chat-a', 'unsent text');
  expect(mocks.invoke).toHaveBeenLastCalledWith('get_context_usage', { chatId: 'chat-a', draft: 'unsent text' });
  await contextBridge.compact('chat-a', '9007199254740993', 'quick', 'request-a');
  expect(mocks.invoke).toHaveBeenLastCalledWith('compact_context', { chatId: 'chat-a', expectedContextRevision: '9007199254740993', mode: 'quick', clientRequestId: 'request-a' });
  await contextBridge.cancel('chat-a', 'job-a'); expect(mocks.invoke).toHaveBeenLastCalledWith('cancel_context_compaction', { chatId: 'chat-a', jobId: 'job-a' });
  await contextBridge.job('chat-a'); expect(mocks.invoke).toHaveBeenLastCalledWith('get_context_compaction', { chatId: 'chat-a' });
});
