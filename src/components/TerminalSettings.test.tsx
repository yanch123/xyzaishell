import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { TerminalSettings } from './TerminalSettings';
import { TERMINAL_SETTINGS_KEY } from '../lib/terminalSettings';
beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });
describe('terminal settings editor', () => {
  it('updates and persists only safe preferences, and restores defaults', () => {
    render(<TerminalSettings />);
    fireEvent.change(screen.getByRole('spinbutton', { name: '终端字体大小' }), { target: { value: '19' } });
    fireEvent.change(screen.getByRole('combobox', { name: '终端回滚行数' }), { target: { value: '1000' } });
    expect(JSON.parse(localStorage.getItem(TERMINAL_SETTINGS_KEY)!)).toEqual({ version: 1, fontSize: 19, scrollback: 1000 });
    fireEvent.click(screen.getByRole('button', { name: '恢复终端默认设置' }));
    expect((screen.getByRole('spinbutton') as HTMLInputElement).value).toBe('14');
    expect((screen.getByRole('combobox') as HTMLSelectElement).value).toBe('5000');
  });
  it('keeps controls usable and warns when storage writes fail', () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('quota'); });
    render(<TerminalSettings />);
    fireEvent.change(screen.getByRole('spinbutton'), { target: { value: '20' } });
    expect((screen.getByRole('spinbutton') as HTMLInputElement).value).toBe('20'); expect(screen.getByRole('alert').textContent).toContain('无法持久保存');
  });
});
