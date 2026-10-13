import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ChatPane } from './ChatPane';
describe('M1 chat shell', () => {
  it('is explicit about unavailable AI and never creates a model reply', async () => {
    const user = userEvent.setup();
    render(<ChatPane showHistory onToggleHistory={vi.fn()} onSettings={vi.fn()} />);
    expect(screen.getByText('AI 尚未接入')).toBeDefined();
    await user.type(screen.getByRole('textbox',{name:'任务草稿'}),'检查服务器'); await user.click(screen.getByRole('button',{name:'保存草稿'}));
    expect(screen.getByText('未发送')).toBeDefined(); expect(screen.getByText('检查服务器',{selector:'p'})).toBeDefined();
    expect(screen.getByText(/没有模型回复或终端执行/)).toBeDefined();
  });
  it('keeps drafts and saved messages separate across chats', async () => {
    const user = userEvent.setup();
    render(<ChatPane showHistory onToggleHistory={vi.fn()} onSettings={vi.fn()} />);
    await user.type(screen.getByRole('textbox'),'未发送草稿 A'); await user.click(screen.getByRole('button',{name:'新建聊天'}));
    expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('');
    await user.type(screen.getByRole('textbox'),'草稿 B'); await user.click(screen.getByRole('button',{name:'新任务 1'}));
    expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('未发送草稿 A');
  });
  it('renders hostile text as text rather than HTML', async () => {
    const user = userEvent.setup();
    render(<ChatPane showHistory={false} onToggleHistory={vi.fn()} onSettings={vi.fn()} />);
    const payload = '<img src=x onerror=alert(1)><script>unsafe</script>';
    fireEvent.change(screen.getByRole('textbox'),{target:{value:payload}}); await user.click(screen.getByRole('button',{name:'保存草稿'}));
    expect(screen.getByText(payload,{selector:'p'})).toBeDefined(); expect(document.querySelector('script')).toBeNull(); expect(document.querySelector('img')).toBeNull();
  });
});
