import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ShellLogDialog } from './ShellLogDialog';
import { shellLogBridge } from '../lib/shellLogBridge';
import type { ShellLogEntry, ShellLogPage, ShellLogStatus } from '../lib/shellLogBridge';
import type { SessionSnapshot } from '../lib/types';
vi.mock('../lib/shellLogBridge', () => ({ shellLogBridge: { status: vi.fn(), start: vi.fn(), pause: vi.fn(), query: vi.fn(), export: vi.fn(), listen: vi.fn() } }));
const session = { session_id: 's', generation: '1' };
const snapshot: SessionSnapshot = { state: 'connected', session };
const status: ShellLogStatus = { enabled: false, available: true, plaintext: true, revision: '1', state: 'disabled', session: null, reason: null, error: null };
const page: ShellLogPage = { entries: [], next_before_seq: null, snapshot_max_seq: '90071992547409999' };
const entry: ShellLogEntry = { id: 'e', sequence: '20', session_id: 's', generation: '1', shell_id: 'sh', shell_sequence: '1', host: 'host', port: 22, username: 'user', started_at_ms: 1, finished_at_ms: null, remote_started_at: '1700', command: '<img src=x onerror=alert(1)>\n=SUM(1)', cwd: '/a b', status: 'running', exit_code: null, redacted: false };
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(r => { resolve = r; }); return { promise, resolve }; }
const open = () => render(<ShellLogDialog native snapshot={snapshot} onClose={vi.fn()} />);
async function consent() { await waitFor(() => expect((screen.getAllByRole('checkbox')[0] as HTMLInputElement).disabled).toBe(false)); for (const checkbox of screen.getAllByRole('checkbox')) fireEvent.click(checkbox); }
describe('independent shell log dialog', () => {
  beforeEach(() => { vi.resetAllMocks(); vi.mocked(shellLogBridge.status).mockResolvedValue(status); vi.mocked(shellLogBridge.listen).mockResolvedValue(vi.fn()); vi.mocked(shellLogBridge.query).mockResolvedValue(page); });
  it('requires both explicit confirmations and prevents duplicate start clicks without a model', async () => {
    const pending = deferred<ShellLogStatus>(); vi.mocked(shellLogBridge.start).mockReturnValue(pending.promise);
    open(); await consent(); const button = screen.getByRole('button', { name: '启动终端日志' });
    fireEvent.click(button); fireEvent.click(button); expect(shellLogBridge.start).toHaveBeenCalledExactlyOnceWith(session);
    await act(async () => pending.resolve({ ...status, revision: '2', session, state: 'starting', enabled: true }));
    expect(screen.getByText(/正在启动（尚未开始记录）/)).toBeDefined(); expect(screen.queryByRole('button', { name: '暂停记录' })).toBeNull();
  });
  it('never auto-starts and displays safe startup failure without pretending enabled', async () => {
    vi.mocked(shellLogBridge.start).mockRejectedValue(new Error('secret transport payload'));
    open(); await consent(); expect(shellLogBridge.start).not.toHaveBeenCalled(); fireEvent.click(screen.getByRole('button', { name: '启动终端日志' }));
    await screen.findByText(/启动结果不确定/); expect(screen.queryByText('secret transport payload')).toBeNull();
    expect(screen.getByText('记录状态：未启用')).toBeDefined(); expect((screen.getByRole('button', { name: '启动终端日志' }) as HTMLButtonElement).disabled).toBe(true);
  });
  it('ignores stale initial status and command results after a newer disconnect event', async () => {
    const initial = deferred<ShellLogStatus>(); vi.mocked(shellLogBridge.status).mockReturnValue(initial.promise);
    let receive!: (s: ShellLogStatus) => void; vi.mocked(shellLogBridge.listen).mockImplementation(async handler => { receive = handler; return () => {}; });
    open(); await act(async () => receive({ ...status, revision: '10', state: 'disconnected', session }));
    await act(async () => initial.resolve({ ...status, revision: '2', state: 'recording', session, enabled: true }));
    expect(screen.getByText('记录状态：连接已断开')).toBeDefined(); expect(screen.queryByRole('button', { name: '暂停记录' })).toBeNull();
  });
  it('disconnect resets confirmation and disables pause even before status event arrives', async () => {
    vi.mocked(shellLogBridge.status).mockResolvedValue({ ...status, session, state: 'recording', enabled: true });
    const view = open(); await screen.findByRole('button', { name: '暂停记录' });
    view.rerender(<ShellLogDialog native snapshot={{ state: 'disconnected', session }} onClose={vi.fn()} />);
    expect(screen.getByText('记录状态：连接已断开')).toBeDefined(); expect(screen.queryByRole('button', { name: '暂停记录' })).toBeNull(); expect(shellLogBridge.pause).not.toHaveBeenCalled();
  });
  it('close and reopen ignores old reads and cleans up a late listener', async () => {
    const old = deferred<ShellLogPage>(); const listener = deferred<() => void>(); const stop = vi.fn();
    vi.mocked(shellLogBridge.query).mockReturnValueOnce(old.promise).mockResolvedValue(page); vi.mocked(shellLogBridge.listen).mockReturnValueOnce(listener.promise);
    const view = open(); view.unmount(); open();
    await act(async () => { old.resolve({ ...page, entries: [entry] }); listener.resolve(stop); });
    expect(stop).toHaveBeenCalledOnce(); expect(screen.queryByText(entry.command)).toBeNull(); expect(shellLogBridge.start).not.toHaveBeenCalled(); expect(shellLogBridge.query).toHaveBeenCalledTimes(2);
  });
  it('uses fixed snapshot keyset cursors, safely renders command text and exports all matching rows', async () => {
    vi.mocked(shellLogBridge.query).mockResolvedValueOnce({ ...page, entries: [entry], next_before_seq: '20' }).mockResolvedValueOnce({ ...page, next_before_seq: null }).mockResolvedValueOnce({ ...page, entries: [entry], next_before_seq: '20' });
    vi.mocked(shellLogBridge.export).mockResolvedValue({ path: '/private/shell-logs/a.csv', records: 81, bytes: 900, format: 'csv' });
    const user = userEvent.setup(); open(); await screen.findByText((_, element) => element?.tagName === 'PRE' && element.textContent === entry.command);
    expect(document.querySelector('img')).toBeNull(); expect(screen.getByText(/本地收到时间/)).toBeDefined(); expect(screen.getByText(/远端报告时间：1700 Unix 秒/)).toBeDefined(); expect(screen.getByText(/已开始，尚未记录结束/)).toBeDefined();
    await user.click(screen.getByRole('button', { name: '下一页' }));
    expect(shellLogBridge.query).toHaveBeenLastCalledWith({ limit: 50, snapshot_max_seq: page.snapshot_max_seq, before_seq: '20' });
    await user.click(screen.getByRole('button', { name: '上一页' }));
    expect(shellLogBridge.query).toHaveBeenLastCalledWith({ limit: 50, snapshot_max_seq: page.snapshot_max_seq, before_seq: null });
    await user.click(screen.getByRole('button', { name: '导出 CSV' }));
    expect(shellLogBridge.export).toHaveBeenCalledExactlyOnceWith({ limit: null, snapshot_max_seq: page.snapshot_max_seq, before_seq: null }, 'csv');
    expect((await screen.findByRole('textbox', { name: '导出文件完整路径' }) as HTMLInputElement).value).toBe('/private/shell-logs/a.csv');
  });
  it('new filter wins over an older outstanding query and clears cursor/snapshot', async () => {
    const old = deferred<ShellLogPage>(); vi.mocked(shellLogBridge.query).mockReturnValueOnce(old.promise).mockResolvedValue({ ...page, snapshot_max_seq: '30' });
    open(); fireEvent.change(screen.getByRole('textbox', { name: '日志关键词' }), { target: { value: 'deploy' } }); fireEvent.click(screen.getByRole('button', { name: '筛选 / 刷新' }));
    await waitFor(() => expect(shellLogBridge.query).toHaveBeenLastCalledWith({ limit: 50, contains: 'deploy' }));
    await act(async () => old.resolve({ ...page, entries: [entry] })); expect(screen.queryByText(entry.command)).toBeNull();
  });
  it('export errors do not show success or retry and duplicate export clicks are locked', async () => {
    const pending = deferred<never>(); vi.mocked(shellLogBridge.export).mockReturnValue(pending.promise);
    open(); await screen.findByText('当前筛选下暂无日志。'); const button = screen.getByRole('button', { name: '导出 JSON' }); fireEvent.click(button); fireEvent.click(button); expect(shellLogBridge.export).toHaveBeenCalledOnce();
  });
  it('reports export failure safely and leaves no stale success path', async () => {
    vi.mocked(shellLogBridge.export).mockRejectedValue(new Error('private-error'));
    open(); await screen.findByText('当前筛选下暂无日志。'); fireEvent.click(screen.getByRole('button', { name: '导出 JSON' }));
    await screen.findByRole('alert'); expect(screen.queryByRole('textbox', { name: '导出文件完整路径' })).toBeNull(); expect(screen.queryByText('private-error')).toBeNull(); expect(shellLogBridge.export).toHaveBeenCalledOnce();
  });
  it('polls starting status until handshake timeout is reported as a gap', async () => {
    vi.useFakeTimers();
    try {
      vi.mocked(shellLogBridge.status).mockResolvedValueOnce({ ...status, state: 'starting', session, revision: '2' }).mockResolvedValue({ ...status, state: 'gap', session, revision: '3', reason: '握手超时' });
      const view = open(); await act(async () => {});
      expect(screen.getByText(/尚未开始记录/)).toBeDefined();
      await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
      expect(screen.getByText(/记录中断.*握手超时/)).toBeDefined();
      await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
      expect(shellLogBridge.status).toHaveBeenCalledTimes(2); view.unmount();
    } finally { vi.useRealTimers(); }
  });
  it('allows pausing enabled Gap capture without restarting the shell', async () => {
    vi.mocked(shellLogBridge.status).mockResolvedValue({ ...status, enabled: true, state: 'gap', session });
    vi.mocked(shellLogBridge.pause).mockResolvedValue({ ...status, enabled: true, state: 'paused', session, revision: '2' });
    open(); fireEvent.click(await screen.findByRole('button', { name: '暂停记录' }));
    expect(shellLogBridge.pause).toHaveBeenCalledExactlyOnceWith(true);
    await screen.findByRole('button', { name: '恢复记录' }); expect(shellLogBridge.start).not.toHaveBeenCalled();
  });
  it('requires SSH reconnection after the integrated shell exits on the same generation', async () => {
    vi.mocked(shellLogBridge.status).mockResolvedValue({ ...status, state: 'disconnected', session });
    open(); await screen.findByText(/日志子 shell 已退出/);
    expect((screen.getByRole('button', { name: '启动终端日志' }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getAllByRole('checkbox')[0] as HTMLInputElement).disabled).toBe(true); expect(shellLogBridge.start).not.toHaveBeenCalled();
  });
  it('web preview performs no native operation or browser persistence', () => {
    render(<ShellLogDialog native={false} snapshot={snapshot} onClose={vi.fn()} />);
    for (const method of Object.values(shellLogBridge)) expect(method).not.toHaveBeenCalled();
    expect((screen.getByRole('button', { name: '启动终端日志' }) as HTMLButtonElement).disabled).toBe(true);
  });
});
