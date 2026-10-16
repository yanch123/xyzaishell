import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ModelEvent } from '../lib/model';
import { chat, modelConfig, stream } from '../test/modelFixtures';
const mocks = vi.hoisted(() => ({ native: vi.fn(), listen: vi.fn(), config: vi.fn(), list: vi.fn(), create: vi.fn(), get: vi.fn(), send: vi.fn(), submission: vi.fn(), cancel: vi.fn(), reload: vi.fn(), confirm: vi.fn(), probe: vi.fn() }));
vi.mock('../lib/bridge', () => ({ desktopAvailable: mocks.native }));
vi.mock('../lib/modelBridge', () => ({ modelBridge: mocks }));
import { ChatPane } from './ChatPane';
import { useModelChat } from '../hooks/useModelChat';
let emit: (event: ModelEvent) => void;
const consent = vi.fn();
function Harness() {
  const model = useModelChat();
  return <ChatPane model={model} showHistory onToggleHistory={vi.fn()} onSettings={vi.fn()} onConsent={consent} />;
}
beforeEach(() => {
  vi.resetAllMocks();
  mocks.native.mockReturnValue(true);
  mocks.listen.mockImplementation(async handler => { emit = handler; return vi.fn(); });
  mocks.config.mockResolvedValue({ ...modelConfig });
  mocks.list.mockResolvedValue([chat()]);
  mocks.create.mockResolvedValue(chat('chat-b'));
  mocks.get.mockImplementation(async id => chat(id));
  mocks.send.mockImplementation(async id => ({ kind: 'chat', chat: stream(id) }));
});
async function ready() {
  render(<Harness />);
  await waitFor(() => expect(screen.getByRole('button', { name: '新建聊天' }).hasAttribute('disabled')).toBe(false));
}
describe('M2 model chat UI', () => {
  it('sends explicit chat text and streams a real backend response without implying terminal access', async () => {
    const user = userEvent.setup(); await ready();
    expect(screen.getByText(/普通命令自动执行，危险操作会先问你/)).toBeDefined();
    await user.type(screen.getByRole('textbox', { name: '聊天消息' }), '解释这条命令');
    await user.click(screen.getByRole('button', { name: '发送' }));
    expect(mocks.send).toHaveBeenCalledWith('chat-a', '解释这条命令', modelConfig.revision, modelConfig.destination, null, expect.any(String));
    expect(screen.getByText('正在等待模型响应…')).toBeDefined();
    act(() => emit({ kind: 'chat_updated', chat: stream('chat-a', '3', 'This is a streamed response.') }));
    expect(screen.getByText('This is a streamed response.')).toBeDefined();
    expect(screen.getByRole('button', { name: '停止生成' })).toBeDefined();
    expect(screen.getByText(/文字回复本身不会执行命令/)).toBeDefined();
  });
  it('asks for consent before a first send instead of calling the provider', async () => {
    mocks.config.mockResolvedValue({ ...modelConfig, consented: false });
    const user = userEvent.setup(); await ready();
    await user.type(screen.getByRole('textbox'), 'message');
    await user.click(screen.getByRole('button', { name: '确认并发送' }));
    expect(consent).toHaveBeenCalledWith('chat-a'); expect(mocks.send).not.toHaveBeenCalled();
    expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('message');
  });
  it('keeps unsent drafts separate when creating and switching chats', async () => {
    const user = userEvent.setup(); await ready();
    await user.type(screen.getByRole('textbox'), '未发送草稿 A');
    await user.click(screen.getByRole('button', { name: '新建聊天' }));
    expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('');
    await user.type(screen.getByRole('textbox'), '草稿 B');
    await user.click(screen.getByRole('button', { name: '新任务 1' }));
    expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('未发送草稿 A');
    expect(mocks.send).not.toHaveBeenCalled(); expect(mocks.cancel).not.toHaveBeenCalled();
  });
  it('renders hostile model and user text as plain text, with no HTML, image or executable links', async () => {
    const payload = '<img src=x onerror=alert(1)><script>unsafe</script> [run](javascript:alert(1))';
    const incoming = stream('chat-a', '2', payload); incoming.messages[0].text = payload;
    mocks.list.mockResolvedValue([incoming]);
    await ready();
    expect(screen.getAllByText(payload, { selector: 'p' })).toHaveLength(2);
    expect(document.querySelector('script')).toBeNull(); expect(document.querySelector('img')).toBeNull(); expect(document.querySelector('a')).toBeNull();
  });
  it('displays incomplete cancellation and stream errors distinctly', async () => {
    const incoming = stream('chat-a', '2', 'partial'); incoming.active_run_id = null; incoming.messages[1].state = 'cancelled';
    mocks.list.mockResolvedValue([incoming]); await ready();
    expect(screen.getByText('已停止 · 回复不完整')).toBeDefined();
    const failed = { ...incoming, revision: '3', messages: incoming.messages.map(message => message.role === 'assistant' ? { ...message, state: 'error' as const, error: { code: 'MODEL_STREAM_INVALID', safe_message: '模型流意外中断，请检查服务。', retry: 'after_user_action' as const } } : message) };
    act(() => emit({ kind: 'chat_updated', chat: failed }));
    expect(screen.getByText('出错 · 回复不完整')).toBeDefined(); expect(screen.getByRole('alert').textContent).toContain('模型流意外中断');
  });
  it('blocks overlong UTF-8 messages without silently trimming text', async () => {
    await ready();
    fireEvent.change(screen.getByRole('textbox'), { target: { value: '中'.repeat(5500) } });
    expect(screen.getByText('消息超过 16 KiB，请缩短后发送')).toBeDefined();
    expect(screen.getByRole('button', { name: '发送' }).hasAttribute('disabled')).toBe(true);
    expect((screen.getByRole('textbox') as HTMLTextAreaElement).value.length).toBe(5500); expect(mocks.send).not.toHaveBeenCalled();
  });
  it('shows an honest browser preview and never simulates a model response', async () => {
    mocks.native.mockReturnValue(false); render(<Harness />);
    expect(screen.getByText('网页预览 · 模型连接不可用')).toBeDefined();
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'not sent' } });
    expect(screen.getByRole('button', { name: '确认并发送' }).hasAttribute('disabled')).toBe(true);
    expect(mocks.send).not.toHaveBeenCalled(); expect(mocks.list).not.toHaveBeenCalled();
  });
});

describe('v0.1.2 immutable chat provenance', () => {
  it('shows restored source identity and keeps historical text read-only', async () => {
    mocks.list.mockResolvedValue([{ ...chat(), source: { host: 'old.example', port: 2222, username: 'original', session_id: 'old-session', generation: '8' }, restored_read_only: true }]);
    await ready();
    expect(screen.getByLabelText('聊天来源').textContent).toContain('original@old.example:2222');
    expect(screen.getByLabelText('聊天来源').textContent).toContain('old-session / 8');
    expect(screen.getByLabelText('聊天来源').textContent).toContain('仅供阅读');
    expect((screen.getByRole('textbox', {name:'聊天消息'}) as HTMLTextAreaElement).disabled).toBe(true);
    expect((screen.getByRole('button',{name:'发送'}) as HTMLButtonElement).disabled).toBe(true);
    expect(mocks.send).not.toHaveBeenCalled();
  });
  it('always separates the live target from a different chat source', async () => {
    mocks.list.mockResolvedValue([{ ...chat(), source: { host: 'old.example', port: 22, username: 'old-user', session_id: 'old-session', generation: '1' } }]);
    function TargetHarness() { const model=useModelChat(); return <ChatPane model={model} showHistory={false} onToggleHistory={vi.fn()} onSettings={vi.fn()} onConsent={consent} session={{state:'connected',session:{session_id:'new-session',generation:'2'},host:'current.example',port:22,username:'new-user'}}/>; }
    render(<TargetHarness/>);
    await waitFor(()=>expect(screen.getByLabelText('聊天来源').textContent).toContain('old-user@old.example:22'));
    expect(screen.getByLabelText('当前 AI 执行目标').textContent).toContain('new-user@current.example:22');
    expect(screen.getByLabelText('聊天来源').textContent).toContain('非当前目标');
    expect((screen.getByRole('textbox',{name:'聊天消息'}) as HTMLTextAreaElement).disabled).toBe(true);
  });
});

describe('terminal attachment review layout', () => {
  it('keeps the preview as its own sibling of the composer and restores normal chat layout on removal', async () => {
    const target = { state: 'connected' as const, host: 'review.example', port: 22, username: 'tester', session: { session_id: 'review-session', generation: '1' } };
    const source = { ...target.session, host: target.host, port: target.port, username: target.username };
    mocks.list.mockResolvedValue([{ ...chat(), source }]);
    function ReviewHarness() {
      const model = useModelChat(target);
      return <><button onClick={() => model.attachSelection('chat-a', { id: 'review-one', source, text: 'selected output', truncated: false })}>Add selection fixture</button><ChatPane model={model} session={target} showHistory={false} onToggleHistory={vi.fn()} onSettings={vi.fn()} onConsent={consent} /></>;
    }
    render(<ReviewHarness />);
    await waitFor(() => expect(screen.getByRole('button', { name: '新建聊天' }).hasAttribute('disabled')).toBe(false));
    const pane = screen.getByRole('complementary', { name: 'AI 聊天工作区' });
    expect(pane.classList.contains('terminal-review')).toBe(false);
    fireEvent.click(screen.getByRole('button', { name: 'Add selection fixture' }));
    const preview = screen.getByRole('region', { name: '终端选区附件草稿' });
    expect(pane.classList.contains('terminal-review')).toBe(true);
    expect(preview.parentElement).toBe(pane);
    const composerArea = screen.getByRole('textbox', { name: '聊天消息' }).closest('.composer-area');
    expect(composerArea?.parentElement).toBe(pane);
    expect(composerArea?.contains(preview)).toBe(false);
    expect(screen.getByRole('button', { name: '发送' }).hasAttribute('disabled')).toBe(true);
    fireEvent.click(screen.getByRole('button', { name: '移除附件' }));
    expect(pane.classList.contains('terminal-review')).toBe(false);
    expect(screen.queryByRole('region', { name: '终端选区附件草稿' })).toBeNull();
    expect(mocks.send).not.toHaveBeenCalled();
  });
});
