import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { ModelConsentDialog } from './ModelConsentDialog';
import { modelConfig } from '../test/modelFixtures';
describe('explicit destination consent', () => {
  it('shows the exact destination, model and data scope and requires a deliberate acknowledgement', async () => {
    const user = userEvent.setup(); const confirm = vi.fn();
    render(<ModelConsentDialog config={modelConfig} busy={false} changed={false} sendAfter error={null} onConfirm={confirm} onClose={vi.fn()} />);
    expect(screen.getByText(modelConfig.destination!)).toBeDefined(); expect(screen.getByText(modelConfig.model!)).toBeDefined();
    expect(screen.getByText(/该聊天已保留的历史/)).toBeDefined();
    const button = screen.getByRole('button', { name: '同意并发送' });
    expect(button.hasAttribute('disabled')).toBe(true);
    await user.click(screen.getByRole('checkbox')); await user.click(button);
    expect(confirm).toHaveBeenCalledOnce();
  });
  it('does not confirm after the configuration changed while the dialog was open', async () => {
    const confirm = vi.fn();
    render(<ModelConsentDialog config={modelConfig} busy={false} changed sendAfter error={null} onConfirm={confirm} onClose={vi.fn()} />);
    expect(screen.getByRole('alert').textContent).toContain('配置已变化');
    fireEvent.click(screen.getByRole('button', { name: '同意并发送' }));
    expect(confirm).not.toHaveBeenCalled();
  });
  it('dismisses via Escape without consenting or clearing the chat', () => {
    const close = vi.fn(); const confirm = vi.fn();
    render(<ModelConsentDialog config={modelConfig} busy={false} changed={false} sendAfter={false} error={null} onConfirm={confirm} onClose={close} />);
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
    expect(close).toHaveBeenCalledOnce(); expect(confirm).not.toHaveBeenCalled();
  });
});
