import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { modelConfig } from '../test/modelFixtures';
import { connectedSession } from '../test/agentFixtures';
import { TerminalTaskStartDialog } from './TerminalTaskStartDialog';
const props = { config: modelConfig, target: connectedSession, instruction: '只检查运行状态', changed: false, busy: false, error: null };
describe('terminal task consent and readiness', () => {
  it('requires two distinct acknowledgements despite pre-existing text-only consent', async () => {
    const onConfirm = vi.fn(); const user = userEvent.setup();
    render(<TerminalTaskStartDialog {...props} onConfirm={onConfirm} onClose={vi.fn()} />);
    expect(screen.getByText(modelConfig.destination!)).toBeDefined(); expect(screen.getByText('test@server.example.test:22')).toBeDefined();
    expect(screen.getByText(/本次运行开始后收集的、有长度上限的终端输出/)).toBeDefined();
    const button = screen.getByRole('button', { name: '同意并开始本次任务' });
    const [consent, readiness] = screen.getAllByRole('checkbox');
    expect(button.hasAttribute('disabled')).toBe(true);
    await user.click(consent); expect(button.hasAttribute('disabled')).toBe(true);
    await user.click(readiness); expect(button.hasAttribute('disabled')).toBe(false);
    await user.click(button); expect(onConfirm).toHaveBeenCalledOnce();
  });
  it('invalidates a previously checked launch when target or model configuration changes', async () => {
    const onConfirm = vi.fn(); const user = userEvent.setup();
    const view = render(<TerminalTaskStartDialog {...props} onConfirm={onConfirm} onClose={vi.fn()} />);
    for (const box of screen.getAllByRole('checkbox')) await user.click(box);
    view.rerender(<TerminalTaskStartDialog {...props} changed onConfirm={onConfirm} onClose={vi.fn()} />);
    expect(screen.getByRole('alert').textContent).toContain('模型配置或 SSH 连接已变化');
    await user.click(screen.getByRole('button', { name: '同意并开始本次任务' }));
    expect(onConfirm).not.toHaveBeenCalled();
  });
  it('dismisses without sending and cannot close during a start mutation', () => {
    const close = vi.fn(); const confirm = vi.fn();
    const view = render(<TerminalTaskStartDialog {...props} onConfirm={confirm} onClose={close} />);
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' }); expect(close).toHaveBeenCalledOnce();
    view.rerender(<TerminalTaskStartDialog {...props} busy onConfirm={confirm} onClose={close} />);
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' }); expect(close).toHaveBeenCalledOnce(); expect(confirm).not.toHaveBeenCalled();
  });
});
