import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { ActionOutput } from './ActionOutput';
import { controller, task } from '../test/agentFixtures';

it('reads bounded local pages only after opening and clicking, without executing commands', async () => {
  const readOutput = vi.fn().mockResolvedValueOnce({ text: 'page one', next_offset: 4096, earliest_offset: 0, complete: false, has_more: true, truncated: false, gap: false }).mockResolvedValueOnce({ text: 'page two', next_offset: 5000, earliest_offset: 0, complete: true, has_more: false, truncated: false, gap: false });
  const start = vi.fn(); const current = task(); const user = userEvent.setup();
  render(<ActionOutput task={current} actionId="action-a" agent={controller({ readOutput, start })} connected />);
  expect(readOutput).not.toHaveBeenCalled();
  await user.click(screen.getByText('查看输出')); await user.click(screen.getByRole('button', { name: '读取输出' }));
  expect(await screen.findByText('page one')).toBeDefined();
  expect(readOutput).toHaveBeenLastCalledWith(current.task_id, current.run_id, 'action-a', 0);
  await user.click(screen.getByRole('button', { name: '下一页' }));
  expect(await screen.findByText('page two')).toBeDefined();
  expect(readOutput).toHaveBeenLastCalledWith(current.task_id, current.run_id, 'action-a', 4096);
  expect(screen.getByRole('button', { name: '下一页' }).hasAttribute('disabled')).toBe(true);
  expect(start).not.toHaveBeenCalled();
});
it('keeps missing output explicit and rejects a late page from an old connection', async () => {
  let resolve!: (page: {text: string; next_offset: number; earliest_offset: number; complete: boolean; has_more: boolean; truncated: boolean; gap: boolean}) => void;
  const readOutput = vi.fn().mockImplementation(() => new Promise(done => { resolve = done; }));
  const current = task(); const agent = controller({ readOutput }); const user = userEvent.setup();
  const view = render(<ActionOutput task={current} actionId="action-a" agent={agent} connected />);
  await user.click(screen.getByText('查看输出')); await user.click(screen.getByRole('button', { name: '读取输出' }));
  view.rerender(<ActionOutput task={current} actionId="action-a" agent={agent} connected={false} />);
  resolve({ text: 'late old output', next_offset: 100, earliest_offset: 0, complete: true, has_more: false, truncated: false, gap: false });
  await waitFor(() => expect(screen.getByRole('button', { name: '读取输出' }).hasAttribute('disabled')).toBe(true));
  expect(screen.queryByText('late old output')).toBeNull();
});
it('shows retained-output gaps instead of implying a complete result', async () => {
  const readOutput = vi.fn().mockResolvedValue({ text: '', next_offset: 8192, earliest_offset: 8192, complete: false, has_more: true, truncated: true, gap: true });
  const user = userEvent.setup();
  render(<ActionOutput task={task()} actionId="action-a" agent={controller({ readOutput })} connected />);
  await user.click(screen.getByText('查看输出')); await user.click(screen.getByRole('button', { name: '读取输出' }));
  expect(await screen.findByText('仅保留部分输出，较早内容可能已丢失。')).toBeDefined();
  expect(screen.getByRole('button', { name: '从可用内容开始' })).toBeDefined();
});

it('pages every retained byte of a completed output larger than 16 KiB', async () => {
  const output = 'A'.repeat(4096) + 'B'.repeat(4096) + 'C'.repeat(4096) + 'D'.repeat(4096) + 'E'.repeat(713);
  const readOutput = vi.fn().mockImplementation(async (_task, _run, _action, offset: number) => {
    const next = Math.min(offset + 4096, output.length);
    return { text: output.slice(offset, next), next_offset: next, earliest_offset: 0,
      complete: true, has_more: next < output.length, truncated: false, gap: false };
  });
  const start = vi.fn(); const user = userEvent.setup();
  const view = render(<ActionOutput task={task({ phase: 'completed' })} actionId="action-a" agent={controller({ readOutput, start })} connected />);
  await user.click(screen.getByText('查看输出'));
  await user.click(screen.getByRole('button', { name: '读取输出' }));
  let collected = '';
  for (let index = 0; index < 5; index++) {
    await waitFor(() => expect(view.container.querySelector('pre')?.textContent).toBe(output.slice(index * 4096, (index + 1) * 4096)));
    collected += view.container.querySelector('pre')!.textContent;
    expect(screen.getByRole('button', { name: '下一页' }).hasAttribute('disabled')).toBe(index === 4);
    if (index < 4) await user.click(screen.getByRole('button', { name: '下一页' }));
  }
  expect(collected).toBe(output);
  expect(readOutput.mock.calls.map(call => call[3])).toEqual([0, 4096, 8192, 12288, 16384]);
  expect(screen.queryByRole('button', { name: '刷新输出' })).toBeNull();
  expect(start).not.toHaveBeenCalled();
});

it('refreshes incomplete output only on explicit local read', async () => {
  const readOutput = vi.fn().mockResolvedValueOnce({ text: 'partial', next_offset: 7, earliest_offset: 0, complete: false, has_more: false, truncated: false, gap: false })
    .mockResolvedValueOnce({ text: 'partial complete', next_offset: 16, earliest_offset: 0, complete: true, has_more: false, truncated: false, gap: false });
  const user = userEvent.setup();
  render(<ActionOutput task={task()} actionId="action-a" agent={controller({ readOutput })} connected />);
  await user.click(screen.getByText('查看输出'));
  await user.click(screen.getByRole('button', { name: '读取输出' }));
  expect(await screen.findByText('partial')).toBeDefined();
  expect(readOutput).toHaveBeenCalledTimes(1);
  expect(screen.getByRole('button', { name: '下一页' }).hasAttribute('disabled')).toBe(true);
  await user.click(screen.getByRole('button', { name: '刷新输出' }));
  expect(await screen.findByText('partial complete')).toBeDefined();
  expect(readOutput.mock.calls.map(call => call[3])).toEqual([0, 0]);
  expect(screen.queryByRole('button', { name: '刷新输出' })).toBeNull();
});
