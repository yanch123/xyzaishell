import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AgentEvent } from '../lib/agent';
import { chat, modelConfig } from '../test/modelFixtures';
import { action, connectedSession, task } from '../test/agentFixtures';
const model = vi.hoisted(() => ({ listen: vi.fn(), config: vi.fn(), list: vi.fn(), create: vi.fn(), get: vi.fn(), send: vi.fn(), cancel: vi.fn(), reload: vi.fn(), confirm: vi.fn(), probe: vi.fn() }));
const agent = vi.hoisted(() => ({ listen: vi.fn(), list: vi.fn(), get: vi.fn(), start: vi.fn(), ack: vi.fn(), decide: vi.fn(), cancel: vi.fn(), continue: vi.fn() }));
vi.mock('../lib/bridge', () => ({ desktopAvailable: () => true }));
vi.mock('../lib/modelBridge', () => ({ modelBridge: model }));
vi.mock('../lib/agentBridge', () => ({ agentBridge: agent }));
vi.mock('../hooks/useSshSession', async () => {
  const { connectedSession: snapshot } = await import('../test/agentFixtures');
  return { useSshSession: () => ({ native: true, ready: true, snapshot, error: null, challenge: null, historyUnavailable: false, size: { current: { cols: 80, rows: 24 } }, disconnect: vi.fn(), connect: vi.fn(), subscribeOutput: vi.fn(), reportError: vi.fn(), setError: vi.fn(), confirmHostKey: vi.fn() }) };
});
vi.mock('./TerminalPane', () => ({ TerminalPane: () => <section aria-label="SSH 终端">手动终端保持可用</section> }));
import App from '../App';
let emit: (event: AgentEvent) => void;
const boundChat = (id = 'chat-a') => ({ ...chat(id), source: { host: connectedSession.host!, port: connectedSession.port!, username: connectedSession.username!, session_id: connectedSession.session!.session_id, generation: connectedSession.session!.generation } });
beforeEach(() => {
  vi.resetAllMocks();
  model.listen.mockResolvedValue(vi.fn()); model.config.mockResolvedValue(modelConfig); model.list.mockResolvedValue([boundChat(), boundChat('chat-b')]); model.get.mockImplementation(async id => boundChat(id)); model.create.mockResolvedValue(boundChat());
  agent.listen.mockImplementation(async handler => { emit = handler; return vi.fn(); }); agent.list.mockResolvedValue([]); agent.get.mockResolvedValue(null);
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({ x: 0, y: 0, top: 0, bottom: 600, left: 0, right: 400, width: 400, height: 600, toJSON: () => ({}) });
});
afterEach(() => vi.restoreAllMocks());
async function ready() {
  render(<App />);
  await waitFor(() => expect(screen.getByRole('button', { name: '新建聊天' }).hasAttribute('disabled')).toBe(false));
}
describe('M3 integrated execution UI', () => {
  it('keeps text chat separate and requires per-run consent before display ACK can dispatch', async () => {
    const user = userEvent.setup(); const prepared = task({ phase: 'awaiting_display', action: action() });
    agent.start.mockResolvedValue(prepared);
    agent.ack.mockImplementation(async () => {
      expect(screen.queryByRole('dialog')).toBeNull();
      expect(screen.getByLabelText('准确命令文本').textContent).toBe('uname -a');
      expect(screen.getAllByText('test@server.example.test:22').length).toBeGreaterThan(0);
      return { ...prepared, revision: '2', phase: 'needs_user', terminal_ready: false, remote_outcome: 'unknown', action: { ...prepared.action!, displayed: true, delivery: 'write_accepted' } };
    });
    await ready();
    await user.type(screen.getByRole('textbox', { name: '聊天消息' }), '这是讨论草稿');
    await user.click(screen.getByRole('tab', { name: '执行终端任务' }));
    expect((screen.getByRole('textbox', { name: '终端任务指令' }) as HTMLTextAreaElement).value).toBe('');
    await user.type(screen.getByRole('textbox', { name: '终端任务指令' }), '检查服务器状态');
    await user.click(screen.getByRole('button', { name: '核对并开始执行' }));
    expect(agent.start).not.toHaveBeenCalled(); expect(agent.ack).not.toHaveBeenCalled();
    const confirm = screen.getByRole('button', { name: '同意并开始本次任务' });
    await user.click(screen.getAllByRole('checkbox')[0]); expect(confirm.hasAttribute('disabled')).toBe(true);
    await user.click(screen.getAllByRole('checkbox')[1]); await user.click(confirm);
    await waitFor(() => expect(agent.ack).toHaveBeenCalledOnce());
    expect(agent.start).toHaveBeenCalledExactlyOnceWith({ chat_id: 'chat-a', session: connectedSession.session, instruction: '检查服务器状态', readiness_confirmed: true, context_consent: { revision: modelConfig.revision, destination: modelConfig.destination } });
    expect(agent.start.mock.invocationCallOrder[0]).toBeLessThan(agent.ack.mock.invocationCallOrder[0]);
    expect(await screen.findByText('SSH 接受写入 · 不代表命令成功')).toBeDefined();
    await user.click(screen.getByRole('tab', { name: '文字聊天' }));
    expect((screen.getByRole('textbox', { name: '聊天消息' }) as HTMLTextAreaElement).value).toBe('这是讨论草稿');
    expect(model.send).not.toHaveBeenCalled(); expect(agent.continue).not.toHaveBeenCalled();
  });
  it('requires the dangerous background action card to be visible before approval, without restarting', async () => {
    const user = userEvent.setup();
    await ready();
    await user.click(screen.getByRole('tab', { name: '执行终端任务' }));
    const prepared = task({ chat_id: 'chat-b', task_id: 'chat-b', phase: 'awaiting_display', action: action({ policy: 'require_approval', risk: 'dangerous', approval_id: 'approval-b' }) });
    act(() => emit({ kind: 'task_updated', task: prepared }));
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 60)); });
    expect(agent.ack).not.toHaveBeenCalled();
    expect(screen.getByText(/另一个聊天拥有 AI 写入权/)).toBeDefined();
    agent.ack.mockResolvedValue({ ...prepared, revision: '2', phase: 'waiting_approval', action: { ...prepared.action!, displayed: true, policy: 'require_approval', approval_id: 'approval-b' } });
    await user.click(screen.getByRole('button', { name: '查看正在执行的任务' }));
    await waitFor(() => expect(agent.ack).toHaveBeenCalledOnce());
    expect(agent.start).not.toHaveBeenCalled(); expect(agent.cancel).not.toHaveBeenCalled(); expect(agent.decide).not.toHaveBeenCalled();
    agent.decide.mockResolvedValue({ ...prepared, revision: '3', phase: 'cancelled', owner: false, action: { ...prepared.action!, displayed: true } });
    fireEvent.click(screen.getByRole('button', { name: '拒绝本次命令' }));
    await waitFor(() => expect(agent.decide).toHaveBeenCalledOnce());
  });
  it('records and acknowledges an ordinary background action while another chat is selected', async () => {
    await ready();
    const prepared = task({ chat_id: 'chat-b', task_id: 'chat-b', phase: 'awaiting_display', action: action({ policy: 'auto_normal', risk: 'normal' }) });
    agent.ack.mockResolvedValue({ ...prepared, revision: '2', phase: 'awaiting_completion', terminal_ready: false, action: { ...prepared.action!, displayed: true, delivery: 'write_accepted' } });
    act(() => emit({ kind: 'task_updated', task: prepared }));
    await waitFor(() => expect(agent.ack).toHaveBeenCalledExactlyOnceWith({ task_id: 'chat-b', run_id: prepared.run_id, action_id: prepared.action!.action_id, display_revision: prepared.action!.display_revision }));
    expect(agent.start).not.toHaveBeenCalled();
    expect(agent.decide).not.toHaveBeenCalled();
    expect(agent.continue).not.toHaveBeenCalled();
  });

});
