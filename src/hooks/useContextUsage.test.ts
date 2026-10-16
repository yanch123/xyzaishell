import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { useContextUsage } from './useContextUsage';
import { contextUsage, compactionJob } from '../test/contextFixtures';
import type { ContextEvent } from '../lib/contextBridge';
import type { ContextUsageSnapshot } from '../lib/context';
const mocks = vi.hoisted(() => ({ usage: vi.fn(), job: vi.fn(), compact: vi.fn(), cancel: vi.fn(), listen: vi.fn() }));
vi.mock('../lib/contextBridge', () => ({ contextBridge: mocks }));
let emit: (event: ContextEvent) => void;
beforeEach(() => { vi.resetAllMocks(); mocks.usage.mockResolvedValue(contextUsage()); mocks.job.mockResolvedValue(null); mocks.listen.mockImplementation(async handler => { emit = handler; return vi.fn(); }); });
it('discards a late draft preview after the user types newer text', async () => {
  let resolveOld!: (value: ContextUsageSnapshot) => void;
  mocks.usage.mockImplementation((_chat: string, draft: string) => draft === 'old' ? new Promise(resolve => { resolveOld = resolve; }) : Promise.resolve(contextUsage({ projection_id: 'new-preview', measurement_scope: 'draft_preview' })));
  const { result, rerender } = renderHook(({ draft }) => useContextUsage('chat-a', '1', '1', draft, true), { initialProps: { draft: 'old' } });
  await waitFor(() => expect(resolveOld).toBeDefined()); rerender({ draft: 'new' });
  await waitFor(() => expect(result.current.snapshot?.projection_id).toBe('new-preview'));
  await act(async () => resolveOld(contextUsage({ projection_id: 'old-preview', measurement_scope: 'draft_preview' })));
  expect(result.current.snapshot?.projection_id).toBe('new-preview'); expect(mocks.compact).not.toHaveBeenCalled();
});
it('rejects previous model snapshots and uses authoritative phase instead of guessing from tool pauses', async () => {
  const { result, rerender } = renderHook(({ config }) => useContextUsage('chat-a', '1', config, '', true), { initialProps: { config: '1' } });
  await waitFor(() => expect(result.current.snapshot).not.toBeNull()); rerender({ config: '2' });
  act(() => emit({ kind: 'context_usage_updated', usage: contextUsage({ context_revision: '99' }) })); expect(result.current.snapshot).toBeNull();
  act(() => emit({ kind: 'context_usage_updated', usage: contextUsage({ config_revision: '2', turn_phase: 'active_turn' }) }));
  expect(result.current.snapshot?.budget.effective_trigger_ratio).toBe(.95);
});
it('committed updates fence older previews even when a draft is visible', async () => {
  const { result } = renderHook(() => useContextUsage('chat-a', '1', '1', 'draft', true));
  await waitFor(() => expect(result.current.snapshot).not.toBeNull());
  mocks.usage.mockResolvedValue(contextUsage({ context_revision: '1', projection_id: 'stale-preview', measurement_scope: 'draft_preview' }));
  act(() => emit({ kind: 'context_usage_updated', usage: contextUsage({ context_revision: '3' }) }));
  await waitFor(() => expect(result.current.committed?.context_revision).toBe('3'));
  expect(result.current.snapshot?.projection_id).not.toBe('stale-preview');
});
it('does not duplicate concurrent manual compactions and reads recover uncertain results', async () => {
  mocks.compact.mockRejectedValue(new Error('response lost'));
  const { result } = renderHook(() => useContextUsage('chat-a', '1', '1', '', true));
  await waitFor(() => expect(result.current.snapshot).not.toBeNull());
  await act(async () => { await Promise.all([result.current.compact('llm'), result.current.compact('quick')]); });
  expect(mocks.compact).toHaveBeenCalledTimes(1); expect(result.current.uncertain).toBe(true);
  mocks.job.mockResolvedValue(compactionJob({ job_id: mocks.compact.mock.calls[0][3], state: 'succeeded', revision: '4' })); await act(async () => result.current.refresh());
  expect(result.current.job?.state).toBe('succeeded'); expect(result.current.uncertain).toBe(false); expect(mocks.compact).toHaveBeenCalledTimes(1);
});
it('blocks a click immediately after chat revision changes until a matching read completes', async () => {
  const { result, rerender } = renderHook(({ revision }) => useContextUsage('chat-a', revision, '1', '', true), { initialProps: { revision: '1' } });
  await waitFor(() => expect(result.current.fresh).toBe(true));
  let resolve!: (usage: ContextUsageSnapshot) => void;
  mocks.usage.mockImplementation(() => new Promise(done => { resolve = done; }));
  rerender({ revision: '2' });
  expect(result.current.fresh).toBe(false);
  await act(async () => result.current.compact('llm'));
  expect(mocks.compact).not.toHaveBeenCalled();
  await waitFor(() => expect(resolve).toBeDefined());
  // A later chat revision invalidates the in-flight read as well.
  rerender({ revision: '3' });
  await act(async () => resolve(contextUsage({ context_revision: '2' })));
  expect(result.current.fresh).toBe(false);
  mocks.usage.mockResolvedValue(contextUsage({ context_revision: '3' }));
  await act(async () => result.current.refresh());
  expect(result.current.fresh).toBe(true);
  mocks.compact.mockResolvedValue(compactionJob());
  await act(async () => result.current.compact('llm'));
  expect(mocks.compact).toHaveBeenCalledWith('chat-a', '3', 'llm', expect.any(String));
});
it('keeps a known stale-action failure visible after polling succeeds, without automatically retrying', async () => {
  mocks.job.mockResolvedValue(compactionJob({ state: 'succeeded', revision: '5' }));
  mocks.compact.mockRejectedValue({ code: 'CONTEXT_STALE', retry: 'after_user_action', safe_message: '上下文已变化，请重新核对后重试。' });
  const { result } = renderHook(() => useContextUsage('chat-a', '1', '1', '', true));
  await waitFor(() => expect(result.current.fresh).toBe(true));
  await act(async () => result.current.compact('llm'));
  await act(async () => result.current.refresh());
  expect(result.current.operationError).toBe('上下文已变化，请重新核对后重试。');
  expect(result.current.uncertain).toBe(false);
  expect(mocks.compact).toHaveBeenCalledTimes(1);
});
it('an unrelated old successful job cannot unlock a lost compaction response', async () => {
  mocks.job.mockResolvedValue(compactionJob({ job_id: 'old-quick', state: 'succeeded', revision: '5' }));
  mocks.compact.mockRejectedValue(new Error('lost response'));
  const { result } = renderHook(() => useContextUsage('chat-a', '1', '1', '', true));
  await waitFor(() => expect(result.current.fresh).toBe(true));
  await act(async () => result.current.compact('llm'));
  await act(async () => result.current.refresh());
  expect(result.current.uncertain).toBe(true); expect(result.current.operationError).not.toBeNull();
  await act(async () => result.current.compact('llm'));
  expect(mocks.compact).toHaveBeenCalledTimes(1);
  mocks.job.mockResolvedValue(compactionJob({ job_id: mocks.compact.mock.calls[0][3], state: 'succeeded', revision: '6' }));
  await act(async () => result.current.refresh());
  expect(result.current.uncertain).toBe(false); expect(result.current.operationError).toBeNull();
});
it('allows manual queuing during an active turn once its current snapshot is fresh', async () => {
  mocks.usage.mockResolvedValue(contextUsage({ turn_phase: 'active_turn' }));
  mocks.compact.mockResolvedValue(compactionJob({ state: 'waiting_safe_point' }));
  const { result } = renderHook(() => useContextUsage('chat-a', '12', '1', '', true));
  await waitFor(() => expect(result.current.fresh).toBe(true));
  await act(async () => result.current.compact('llm'));
  expect(mocks.compact).toHaveBeenCalledTimes(1); expect(result.current.job?.state).toBe('waiting_safe_point');
});
it('treats a typed memory quota rejection as definitive, keeping its error until explicit retry', async () => {
  mocks.compact.mockRejectedValueOnce({ code: 'CONTEXT_MEMORY_LIMIT', safe_message: '上下文内存不足，请先减少其他会话。', retry: 'after_user_action' }).mockResolvedValueOnce(compactionJob());
  const { result } = renderHook(() => useContextUsage('chat-a', '1', '1', '', true));
  await waitFor(() => expect(result.current.fresh).toBe(true));
  await act(async () => result.current.compact('llm'));
  await act(async () => result.current.refresh());
  expect(result.current.uncertain).toBe(false);
  expect(result.current.operationError).toBe('上下文内存不足，请先减少其他会话。');
  expect(result.current.fresh).toBe(true); expect(mocks.compact).toHaveBeenCalledTimes(1);
  await act(async () => result.current.compact('llm'));
  expect(mocks.compact).toHaveBeenCalledTimes(2); expect(result.current.operationError).toBeNull();
});
it('an incomplete error-shaped object is still an unknown submission outcome', async () => {
  mocks.compact.mockRejectedValue({ code: 'CONTEXT_MEMORY_LIMIT', safe_message: 'Incomplete transport object' });
  const { result } = renderHook(() => useContextUsage('chat-a', '1', '1', '', true));
  await waitFor(() => expect(result.current.fresh).toBe(true));
  await act(async () => result.current.compact('llm'));
  await act(async () => result.current.refresh());
  expect(result.current.uncertain).toBe(true);
  await act(async () => result.current.compact('llm')); expect(mocks.compact).toHaveBeenCalledTimes(1);
});
