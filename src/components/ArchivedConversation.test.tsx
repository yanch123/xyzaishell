import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { ArchivedConversation } from './ArchivedConversation';
import { chat, stream } from '../test/modelFixtures';
const mocks = vi.hoisted(() => ({ list: vi.fn(), page: vi.fn() }));
vi.mock('../lib/storageBridge', () => ({ historyBridge: mocks }));
it('reads bounded archive pages without injecting them into the active conversation', async () => {
  mocks.list.mockResolvedValue([{ chat_id: 'chat-a', revision: '7' }]);
  mocks.page.mockResolvedValueOnce({ chat: { ...stream('chat-a', '7', '最新原文'), active_run_id: null, restored_read_only: true }, total_messages: 300, next_before: 236 }).mockResolvedValueOnce({ chat: { ...chat(), restored_read_only: true, messages: [{ ...stream().messages[0], text: '早期原文' }] }, total_messages: 300, next_before: 172 });
  render(<ArchivedConversation chatId="chat-a" />); fireEvent.click(screen.getByText('翻阅已保存对话')); await screen.findByText('最新原文'); fireEvent.click(screen.getByText('更早一页')); await screen.findByText('早期原文');
  expect(mocks.page).toHaveBeenLastCalledWith('chat-a', '7', 236); expect(screen.queryByText('最新原文')).toBeNull(); expect(screen.getByText(/不加入模型上下文/)).toBeDefined();
  fireEvent.click(screen.getByText('收起历史页')); await waitFor(() => expect(screen.queryByRole('log')).toBeNull());
});
