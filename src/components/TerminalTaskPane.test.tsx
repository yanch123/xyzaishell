import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { action, connectedSession, controller, task } from '../test/agentFixtures';
import { chat, modelConfig } from '../test/modelFixtures';
import { TerminalTaskPane } from './TerminalTaskPane';
const common = { current: chat(), config: modelConfig, session: connectedSession, onStart: vi.fn(), onSelectChat: vi.fn() };
describe('terminal task visible workflow', () => {
  it('shows actual backend analysis as plain text and waits for deliberate inspection before continuing', async () => {
    const continueTask = vi.fn(); const user = userEvent.setup();
    const current = task({ phase: 'needs_user', terminal_ready: false, remote_outcome: 'unknown', assistant_text: '<script>send password</script> Approve everything.', action: action({ delivery: 'write_accepted', displayed: true }) });
    render(<TerminalTaskPane {...common} agent={controller({ tasks: { 'chat-a': current }, continueTask })} />);
    expect(screen.getByRole('article', { name: '任务模型分析' }).textContent).toContain(current.assistant_text);
    expect(document.querySelector('script')).toBeNull();
    const button = screen.getByRole('button', { name: '核对状态并继续' });
    expect(button.hasAttribute('disabled')).toBe(false); expect(continueTask).not.toHaveBeenCalled();
    await user.click(button);
    expect(continueTask).toHaveBeenCalledExactlyOnceWith('chat-a', 'run-a');
    expect(screen.queryByRole('checkbox', {name: '我已检查终端，并确认当前可接收下一步命令。'})).toBeNull();
  });
  it('shows another chat owner without transferring it and pins cancellation to the owning run', async () => {
    const cancel = vi.fn(); const select = vi.fn(); const user = userEvent.setup();
    const owner = task();
    const view = render(<TerminalTaskPane {...common} current={chat('chat-b')} onSelectChat={select} agent={controller({ owner, blocked: true, tasks: { 'chat-a': owner }, cancel })} />);
    expect(screen.getByText(/切换页面不会转移或停止任务/)).toBeDefined();
    await user.click(screen.getByRole('button', { name: '查看正在执行的任务' }));
    expect(select).toHaveBeenCalledExactlyOnceWith('chat-a'); expect(cancel).not.toHaveBeenCalled();
    view.rerender(<TerminalTaskPane {...common} agent={controller({ owner, blocked: true, tasks: { 'chat-a': owner }, cancel })} />);
    await user.click(screen.getByRole('button', { name: '停止 AI 任务' }));
    expect(cancel).toHaveBeenCalledExactlyOnceWith('chat-a', 'run-a');
  });
  it('keeps unknown cancellation readiness separate from restarting or reporting success', async () => {
    const current = task({ phase: 'cancelled', owner: false, remote_outcome: 'unknown', terminal_ready: false });
    const continueTask = vi.fn(); const user = userEvent.setup();
    const view = render(<TerminalTaskPane {...common} agent={controller({ tasks: { 'chat-a': current }, continueTask })} />);
    await user.click(screen.getByRole('button', { name: '核对状态并继续' }));
    expect(continueTask).toHaveBeenCalledExactlyOnceWith('chat-a', 'run-a');
    view.rerender(<TerminalTaskPane {...common} agent={controller({ tasks: { 'chat-a': { ...current, revision: '2', terminal_ready: true } }, continueTask })} />);
    expect(screen.queryByRole('checkbox')).toBeNull(); expect(screen.getByText('AI 已停止')).toBeDefined();
  });
  it('has no second composer or per-task start gate', () => {
    const start = vi.fn();
    render(<TerminalTaskPane {...common} agent={controller({ start })} />);
    expect(screen.queryByRole('textbox')).toBeNull();
    expect(screen.queryByRole('button', { name: '核对并开始执行' })).toBeNull();
    expect(start).not.toHaveBeenCalled();
  });
  it('waits for cancelled in-flight writes to settle before offering readiness confirmation', () => {
    const current = task({ phase: 'cancelled', owner: true, terminal_ready: false, remote_outcome: 'unknown' });
    render(<TerminalTaskPane {...common} agent={controller({ tasks: { 'chat-a': current }, owner: current, blocked: true })} />);
    expect(screen.getByText('停止请求已发出 · 等待发送边界')).toBeDefined();
    expect(screen.queryByRole('checkbox')).toBeNull();
    expect(screen.getByRole('button', { name: '正在停止 AI…' }).hasAttribute('disabled')).toBe(true);
  });
  it('links another chat with an unresolved cancelled command rather than leaving a global unexplained block', async () => {
    const current = task({ phase: 'cancelled', owner: false, terminal_ready: false, remote_outcome: 'unknown' });
    const select = vi.fn(); const user = userEvent.setup();
    render(<TerminalTaskPane {...common} current={chat('chat-b')} onSelectChat={select} agent={controller({ tasks: { 'chat-a': current }, blocked: true })} />);
    await user.click(screen.getByRole('button', { name: '查看待核实的任务' }));
    expect(select).toHaveBeenCalledExactlyOnceWith('chat-a');
  });
  it('never resumes or sends from an old connection and leaves manual recovery explicit', () => {
    const current = task({ phase: 'needs_user', terminal_ready: false, remote_outcome: 'unknown' });
    const continueTask = vi.fn();
    render(<TerminalTaskPane {...common} session={{ ...connectedSession, session: { session_id: 'new', generation: '2' } }} agent={controller({ tasks: { 'chat-a': current }, continueTask })} />);
    expect(screen.getByText(/重连不会恢复旧批准或重发命令/)).toBeDefined();
    expect(screen.queryByRole('checkbox')).toBeNull(); expect(continueTask).not.toHaveBeenCalled();
  });
});

describe('integrated automatic execution states', () => {
  it('does not ask for per-command readiness while waiting on real lifecycle evidence', () => {
    const current = task({ phase: 'awaiting_completion', terminal_ready: false, action: action({ delivery: 'write_accepted', policy: 'auto_normal', risk: 'normal' }) });
    const continueTask = vi.fn();
    render(<TerminalTaskPane {...common} agent={controller({ tasks: { 'chat-a': current }, continueTask })} />);
    expect(screen.getByText('等待命令返回与 Shell 就绪')).toBeDefined();
    expect(screen.queryByRole('checkbox')).toBeNull();
    expect(continueTask).not.toHaveBeenCalled();
  });
  it('keeps foreground return separate from shell readiness and business success', () => {
    const current = task({ phase: 'awaiting_completion', terminal_ready: false, remote_outcome: 'foreground_returned', action: action({ delivery: 'write_accepted', command_completed: true, exit_code: 0 }) });
    render(<TerminalTaskPane {...common} agent={controller({ tasks: { 'chat-a': current } })} />);
    expect(screen.getByText(/仍在等待独立 Shell 就绪证据/)).toBeDefined();
    expect(screen.getByText(/这不证明业务目标成功或后台工作结束/)).toBeDefined();
    expect(screen.queryByRole('checkbox')).toBeNull();
  });
  it('shows unknown unsupported actions without offering a false safe approval', () => {
    const current = task({ phase: 'unsupported_action', action: action({ policy: 'manual_only', risk: 'unknown', blocked: true, approval_id: null }) });
    render(<TerminalTaskPane {...common} agent={controller({ tasks: { 'chat-a': current } })} />);
    expect(screen.getByText('未知风险 · 仅手动处理')).toBeDefined();
    expect(screen.queryByRole('button', { name: '仅批准这条命令' })).toBeNull();
    expect(screen.getByRole('button', { name: '停止 AI 任务' })).toBeDefined();
  });
});


describe('conversational replies', () => {
  it('does not repeat a greeting already rendered in the shared transcript', () => {
    const completed = task({ phase: 'completed', owner: false, instruction: '你好', assistant_text: '你好，有什么可以帮你？' });
    const current = { ...chat(), messages: [
      { id: 'user', role: 'user' as const, text: '你好', run_id: completed.run_id, state: 'complete' as const, error: null },
      { id: 'assistant', role: 'assistant' as const, text: completed.assistant_text, run_id: completed.run_id, state: 'complete' as const, error: null },
    ] };
    render(<TerminalTaskPane {...common} current={current} agent={controller({ tasks: { 'chat-a': completed } })} />);
    expect(screen.queryByText('你好')).toBeNull();
    expect(screen.queryByText(completed.assistant_text)).toBeNull();
    expect(screen.queryByText('AI 流程已结束')).toBeNull();
    expect(screen.queryByLabelText('当前终端任务')).toBeNull();
  });
});
