import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { ContextIndicator } from './ContextIndicator';
import { chat, modelConfig } from '../test/modelFixtures';
import { contextUsage, compactionJob } from '../test/contextFixtures';
import type { ModelChatController } from '../hooks/useModelChat';
import type { ContextEvent } from '../lib/contextBridge';
const mocks = vi.hoisted(() => ({ usage: vi.fn(), job: vi.fn(), compact: vi.fn(), cancel: vi.fn(), listen: vi.fn() }));
vi.mock('../lib/contextBridge', () => ({ contextBridge: mocks }));
let emit: (event: ContextEvent) => void;
const save = vi.fn();
function setup(draft = '') { return render(<ContextIndicator chat={chat()} draft={draft} model={{ native: true, ready: true, config: modelConfig, saveConfig: save } as unknown as ModelChatController} readOnly={false} onSettings={vi.fn()} />); }
beforeEach(() => { vi.resetAllMocks(); mocks.usage.mockResolvedValue(contextUsage()); mocks.job.mockResolvedValue(null); mocks.compact.mockResolvedValue(compactionJob()); mocks.cancel.mockResolvedValue(compactionJob({ revision: '3', state: 'cancelled' })); mocks.listen.mockImplementation(async handler => { emit = handler; return vi.fn(); }); save.mockResolvedValue(modelConfig); });
it('uses input budget occupancy and shows categories, default source, phase and unknown reported usage honestly', async () => {
  setup(); await waitFor(() => expect(screen.getByRole('button', { name: /可用输入预算占用 ≈5%/ })).toBeDefined());
  fireEvent.click(screen.getByRole('button', { name: /查看上下文/ }));
  expect(screen.getByText(/按默认容量估算/)).toBeDefined(); expect(screen.getByText(/当前任务中 95%/)).toBeDefined(); expect(screen.getByText(/占输入 80%/)).toBeDefined();
  fireEvent.click(screen.getByText('详情')); expect(screen.getAllByText('未知 · 未收到供应商用量')).toHaveLength(2);
  expect((screen.getByRole('checkbox', { name: '自动压缩' }) as HTMLInputElement).checked).toBe(true);
  expect((screen.getByLabelText('自动压缩方式') as HTMLSelectElement).value).toBe('llm');
});
it('supports keyboard opening, Escape focus restoration, and closing does not cancel', async () => {
  const user = userEvent.setup(); setup(); const trigger = screen.getByRole('button', { name: /查看上下文/ }); trigger.focus(); await user.keyboard('{Enter}');
  expect(screen.getByRole('dialog')).toBeDefined(); await user.keyboard('{Escape}'); expect(screen.queryByRole('dialog')).toBeNull(); expect(document.activeElement).toBe(trigger); expect(mocks.cancel).not.toHaveBeenCalled();
});
it('manual quick neither changes auto mode nor sends draft; repeated click creates one job and cancel does not revive', async () => {
  setup('未发送的秘密草稿'); await waitFor(() => expect(mocks.usage).toHaveBeenCalledWith('chat-a', '未发送的秘密草稿'));
  fireEvent.click(screen.getByRole('button', { name: /查看上下文/ })); fireEvent.change(screen.getByLabelText('本次压缩方式'), { target: { value: 'quick' } });
  const compact = screen.getByRole('button', { name: '立即压缩' }); fireEvent.click(compact); fireEvent.click(compact);
  await waitFor(() => expect(mocks.compact).toHaveBeenCalledTimes(1)); expect(mocks.compact).toHaveBeenCalledWith('chat-a', '1', 'quick', expect.any(String)); expect(save).not.toHaveBeenCalled();
  fireEvent.click(await screen.findByRole('button', { name: '取消压缩' })); await screen.findByText('压缩已取消，原上下文保留');
  act(() => emit({ kind: 'context_compaction_updated', job: compactionJob({ revision: '2' }) })); expect(screen.queryByRole('button', { name: '取消压缩' })).toBeNull();
});
it('unknown capacity never becomes zero percent and over-budget is not clamped', async () => {
  mocks.usage.mockResolvedValue(contextUsage({ budget: { ...contextUsage().budget, input_budget_tokens: null, input_occupancy_ratio: null } })); setup();
  await waitFor(() => expect(mocks.usage).toHaveBeenCalled()); expect(screen.getByRole('button', { name: /可用输入预算占用 —/ })).toBeDefined();
  act(() => emit({ kind: 'context_usage_updated', usage: contextUsage({ context_revision: '2', budget: { ...contextUsage().budget, input_occupancy_ratio: 1.2 } }) }));
  expect(screen.getByRole('button', { name: /≈120%/ })).toBeDefined();
  act(() => emit({ kind: 'context_usage_updated', usage: contextUsage() })); expect(screen.getByRole('button', { name: /≈120%/ })).toBeDefined();
});
it('does not retry unknown compaction outcome or silently select quick after LLM failure', async () => {
  mocks.compact.mockRejectedValue(new Error('IPC disconnected')); setup(); await waitFor(() => expect(mocks.usage).toHaveBeenCalled());
  fireEvent.click(screen.getByRole('button', { name: /查看上下文/ })); fireEvent.click(screen.getByRole('button', { name: '立即压缩' })); await screen.findByText(/操作结果尚未确认/);
  expect(screen.getByRole('button', { name: '重试压缩' }).hasAttribute('disabled')).toBe(true); expect(mocks.compact).toHaveBeenCalledTimes(1);
});
it('saves automatic quick independently without manufacturing a capacity override', async () => {
  setup(); fireEvent.click(screen.getByRole('button', { name: /查看上下文/ }));
  fireEvent.change(screen.getByLabelText('自动压缩方式'), { target: { value: 'quick' } });
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  expect(save).toHaveBeenCalledWith(expect.objectContaining({ context_auto_compact: true, context_auto_mode: 'quick', key_update: { kind: 'keep' } }));
  expect(save.mock.calls[0][0]).not.toHaveProperty('context_window_tokens'); expect(mocks.compact).not.toHaveBeenCalled();
});
it('does not show a previous quick success as the result of a rejected LLM attempt', async () => {
  mocks.job.mockResolvedValue(compactionJob({ job_id: 'old-quick', mode: 'quick', state: 'succeeded', revision: '5' }));
  mocks.compact.mockRejectedValue({ code: 'CONTEXT_STALE', retry: 'after_user_action', safe_message: '上下文已变化，请重新核对后重试。' });
  setup(); await waitFor(() => expect(mocks.usage).toHaveBeenCalled());
  fireEvent.click(screen.getByRole('button', { name: /查看上下文/ }));
  await screen.findByText('上下文已整理');
  fireEvent.click(screen.getByRole('button', { name: '立即压缩' }));
  await screen.findByText('上下文已变化，请重新核对后重试。');
  await waitFor(() => expect(mocks.usage.mock.calls.length).toBeGreaterThan(1));
  expect(screen.queryByText('上下文已整理')).toBeNull();
  expect(screen.getByRole('button', { name: '重试压缩' })).toBeDefined();
  expect(mocks.compact).toHaveBeenCalledTimes(1);
});
