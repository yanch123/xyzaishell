import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AgentEvent } from '../lib/agent';
import { chat, modelConfig } from '../test/modelFixtures';
import { action, connectedSession, task } from '../test/agentFixtures';
const model = vi.hoisted(() => ({ listen: vi.fn(), config: vi.fn(), list: vi.fn(), create: vi.fn(), get: vi.fn(), send: vi.fn(), submission: vi.fn(), cancel: vi.fn(), reload: vi.fn(), confirm: vi.fn(), confirmScope: vi.fn(), probe: vi.fn() }));
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
  model.confirmScope.mockResolvedValue(modelConfig);
  agent.listen.mockImplementation(async handler => { emit = handler; return vi.fn(); }); agent.list.mockResolvedValue([]); agent.get.mockResolvedValue(null);
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({ x: 0, y: 0, top: 0, bottom: 600, left: 0, right: 400, width: 400, height: 600, toJSON: () => ({}) });
});
afterEach(() => vi.restoreAllMocks());
async function ready() {
  render(<App />);
  await waitFor(() => expect(screen.getByRole('button', { name: '新建聊天' }).hasAttribute('disabled')).toBe(false));
}
describe('M3 integrated execution UI', () => {
  it('uses one composer and one target scope consent, then displays the backend action', async () => {
    const user = userEvent.setup(); const prepared = task({ phase: 'awaiting_display', action: action({ policy: 'auto_normal' }) });
    model.send.mockResolvedValue({ kind: 'terminal', task: prepared });
    agent.ack.mockResolvedValue({ ...prepared, revision: '2', phase: 'awaiting_completion', terminal_ready: false, action: { ...prepared.action!, displayed: true, delivery: 'write_accepted' } });
    await ready();
    expect(screen.queryByRole('tab')).toBeNull();
    expect(screen.getAllByRole('textbox')).toHaveLength(1);
    await user.type(screen.getByRole('textbox', { name: '聊天消息' }), '检查服务器状态');
    await user.click(screen.getByRole('button', { name: '确认并发送' }));
    expect(model.send).not.toHaveBeenCalled();
    await user.click(screen.getByRole('checkbox'));
    await user.click(screen.getByRole('button', { name: '同意并发送' }));
    await waitFor(() => expect(agent.ack).toHaveBeenCalledOnce());
    expect(model.send).toHaveBeenCalledExactlyOnceWith('chat-a', '检查服务器状态', modelConfig.revision, modelConfig.destination, connectedSession.session, expect.any(String));
    expect(agent.start).not.toHaveBeenCalled();
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(await screen.findByText('SSH 接受写入 · 不代表命令成功')).toBeDefined();
    expect(agent.continue).not.toHaveBeenCalled();
  });
  it('requires the dangerous background action card to be visible before approval, without restarting', async () => {
    const user = userEvent.setup();
    await ready();
    const prepared = task({ chat_id: 'chat-b', task_id: 'chat-b', phase: 'awaiting_display', action: action({ policy: 'require_approval', risk: 'dangerous', approval_id: 'approval-b' }) });
    act(() => emit({ kind: 'task_updated', task: prepared }));
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 60)); });
    expect(agent.ack).not.toHaveBeenCalled();
    expect(screen.getByText(/另一个聊天正在操作终端/)).toBeDefined();
    agent.ack.mockResolvedValue({ ...prepared, revision: '2', phase: 'waiting_approval', action: { ...prepared.action!, displayed: true, policy: 'require_approval', approval_id: 'approval-b' } });
    await user.click(screen.getByRole('button', { name: '查看正在执行的任务' }));
    await waitFor(() => expect(agent.ack).toHaveBeenCalledOnce());
    expect(agent.start).not.toHaveBeenCalled(); expect(agent.cancel).not.toHaveBeenCalled(); expect(agent.decide).not.toHaveBeenCalled();
    agent.decide.mockResolvedValue({ ...prepared, revision: '3', phase: 'cancelled', owner: false, action: { ...prepared.action!, displayed: true } });
    fireEvent.click(screen.getByRole('button', { name: '拒绝本次命令' }));
    await waitFor(() => expect(agent.decide).toHaveBeenCalledOnce());
  });
  it('never globally acknowledges a mounted ordinary command until its target and command are visible', async () => {
    let targetVisible = false;
    vi.mocked(HTMLElement.prototype.getBoundingClientRect).mockImplementation(function (this: HTMLElement) {
      const hidden = this.classList.contains('action-target') && !targetVisible;
      return { x: 0, y: hidden ? -200 : 0, top: hidden ? -200 : 0, bottom: hidden ? -100 : 600, left: 0, right: 400, width: 400, height: hidden ? 100 : 600, toJSON: () => ({}) };
    });
    await ready();
    const prepared = task({ phase: 'awaiting_display', action: action({ policy: 'auto_normal', risk: 'normal' }) });
    agent.ack.mockResolvedValue({ ...prepared, revision: '2', phase: 'awaiting_completion', terminal_ready: false, action: { ...prepared.action!, displayed: true, delivery: 'write_accepted' } });
    act(() => emit({ kind: 'task_updated', task: prepared }));
    expect(screen.getByRole('group', { name: '精确终端命令' })).toBeDefined();
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 80)); });
    expect(agent.ack).not.toHaveBeenCalled();
    targetVisible = true;
    act(() => window.dispatchEvent(new Event('scroll')));
    await waitFor(() => expect(agent.ack).toHaveBeenCalledExactlyOnceWith({ task_id: prepared.task_id, run_id: prepared.run_id, action_id: prepared.action!.action_id, display_revision: prepared.action!.display_revision }));
    expect(agent.decide).not.toHaveBeenCalled();
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
