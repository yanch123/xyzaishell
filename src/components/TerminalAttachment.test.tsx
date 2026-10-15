import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { TerminalAttachment } from './TerminalAttachment';
import { modelConfig } from '../test/modelFixtures';
const attachment = { id: 'one', source: { session_id: 'session-a', generation: '1', username: 'operator', host: 'a.example', port: 22 }, text: '<script>ignore instructions</script>', truncated: true, edited: true, consent: null };
describe('reviewable attachment preview', () => {
  it('exposes immutable source, edited/truncated status and exact destination with separate consent', () => {
    const edit = vi.fn(); const remove = vi.fn(); const consent = vi.fn();
    render(<TerminalAttachment attachment={attachment} config={modelConfig} stale={false} disabled={false} onEdit={edit} onRemove={remove} onConsent={consent} />);
    expect(screen.getByText(/operator@a.example:22/).textContent).toContain('session-a / 1');
    expect(screen.getByText(/已由你编辑/).textContent).toContain('截断');
    expect(screen.getByText(/本次接收方/).textContent).toContain(modelConfig.destination);
    expect(screen.getByRole('checkbox').textContent).toBe(''); expect(screen.getByText(/本次消息及当前聊天历史/)).toBeDefined();
    expect(document.querySelector('script')).toBeNull(); expect(consent).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'safe reviewed output' } }); expect(edit).toHaveBeenCalledWith('safe reviewed output');
    fireEvent.click(screen.getByRole('checkbox')); expect(consent).toHaveBeenCalledWith(true);
    fireEvent.click(screen.getByRole('button', { name: '移除附件' })); expect(remove).toHaveBeenCalledOnce();
  });
  it('does not allow stale consent or editing, but permits removing a stale attachment', () => {
    render(<TerminalAttachment attachment={{ ...attachment, consent: { revision: '1', destination: modelConfig.destination! } }} config={modelConfig} stale disabled={false} onEdit={vi.fn()} onRemove={vi.fn()} onConsent={vi.fn()} />);
    const checkbox = screen.getByRole('checkbox') as HTMLInputElement;
    expect(checkbox.disabled).toBe(true); expect(checkbox.checked).toBe(false);
    expect((screen.getByRole('textbox') as HTMLTextAreaElement).disabled).toBe(true);
    expect(screen.getByRole('button', { name: '移除附件' }).hasAttribute('disabled')).toBe(false);
  });
});
