import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ConnectionDialog } from './ConnectionDialog';
import { storageBridge } from '../lib/storageBridge';
import type { ProfileStatus, SavedProfile } from '../lib/storageBridge';
vi.mock('../lib/storageBridge', () => ({ storageBridge: { profiles: vi.fn(), saveProfile: vi.fn() } }));
const size = { cols: 80, rows: 24 };
const status: ProfileStatus = { platform: 'linux', configuration_root: '/private', credentials: { credential_backend: 'linux_secret_service', credential_availability: 'available', persistent_credentials: true }, listing: { revision: '0', mode: 'linux_secret_service', connections: [] } };
const saved: SavedProfile = { id: 'profile-1', label: 'Test', host: 'test.example', port: 22, username: 'tester', auth_kind: 'password', private_key_path: null, credential_policy: 'system_secure', credential_present: true };
const fill = async (user: ReturnType<typeof userEvent.setup>) => { await user.type(screen.getByLabelText('主机地址'), 'test.example'); await user.type(screen.getByLabelText('用户名'), 'tester'); await user.type(screen.getByLabelText('登录密码'), 'SYNTHETIC_SECRET'); };
describe('unified connection editor', () => {
  beforeEach(() => { vi.clearAllMocks(); vi.mocked(storageBridge.profiles).mockResolvedValue(status); vi.mocked(storageBridge.saveProfile).mockImplementation(async request => ({ ...status, listing: { ...status.listing, revision: '1', connections: [{ ...request.profile, id: request.profile.id!, credential_policy: request.credential_update.kind === 'set' ? 'system_secure' : 'ask_on_connect', credential_present: request.credential_update.kind === 'set' }] } })); });
  it('defaults to temporary authentication and clears input even after failure', async () => {
    const connect = vi.fn().mockResolvedValue(false); const user = userEvent.setup();
    render(<ConnectionDialog available size={size} onConnect={connect} onClose={vi.fn()} />); await fill(user);
    expect((screen.getByLabelText('保存连接') as HTMLInputElement).checked).toBe(false);
    expect((screen.getByRole('checkbox', { name: /记住密码/ }) as HTMLInputElement).disabled).toBe(true);
    await user.click(screen.getByRole('button', { name: '连接服务器' }));
    expect(connect).toHaveBeenCalledExactlyOnceWith({ host: 'test.example', port: 22, username: 'tester', session_only: true, size, auth: { kind: 'password', password: 'SYNTHETIC_SECRET' } });
    expect(screen.queryByDisplayValue('SYNTHETIC_SECRET')).toBeNull(); expect(storageBridge.saveProfile).not.toHaveBeenCalled(); expect(localStorage.length).toBe(0);
  });
  it('saves metadata before connecting with a one-attempt credential', async () => {
    const connect = vi.fn(); const savedConnect = vi.fn().mockResolvedValue(true); const user = userEvent.setup();
    render(<ConnectionDialog available size={size} onConnect={connect} onConnectSaved={savedConnect} onClose={vi.fn()} />); await fill(user); await user.type(screen.getByLabelText('连接名称'), 'Test'); await user.click(screen.getByLabelText('保存连接')); await user.click(screen.getByRole('button', { name: '连接服务器' }));
    await waitFor(() => expect(savedConnect).toHaveBeenCalledOnce());
    const request = vi.mocked(storageBridge.saveProfile).mock.calls[0][0];
    expect(request.credential_update).toEqual({ kind: 'keep' }); expect(savedConnect).toHaveBeenCalledWith(request.profile.id, '1', { temporaryCredential: 'SYNTHETIC_SECRET' }); expect(connect).not.toHaveBeenCalled();
  });
  it('persists a secret only after selecting remember and stops if save fails', async () => {
    vi.mocked(storageBridge.saveProfile).mockRejectedValueOnce({ code: 'CREDENTIAL_UNAVAILABLE', safe_message: '系统安全存储不可用。' });
    const connect = vi.fn(); const user = userEvent.setup(); render(<ConnectionDialog available size={size} onConnect={connect} onClose={vi.fn()} />); await fill(user); await user.type(screen.getByLabelText('连接名称'), 'Test'); await user.click(screen.getByLabelText('保存连接')); await user.click(screen.getByRole('checkbox', { name: /记住密码/ })); await user.click(screen.getByRole('button', { name: '连接服务器' }));
    await screen.findByRole('alert'); expect(vi.mocked(storageBridge.saveProfile).mock.calls[0][0].credential_update).toEqual({ kind: 'set', value: 'SYNTHETIC_SECRET' }); expect(connect).not.toHaveBeenCalled(); expect(screen.queryByDisplayValue('SYNTHETIC_SECRET')).toBeNull();
  });
  it('edits an existing label while preserving its policy and never connects', async () => {
    const connect = vi.fn(); const close = vi.fn(); const user = userEvent.setup(); render(<ConnectionDialog available size={size} profile={saved} status={{ ...status, listing: { ...status.listing, connections: [saved] } }} onConnect={connect} onClose={close} />);
    expect(screen.getByPlaceholderText('已保存，留空保留')).toBeDefined(); await user.type(screen.getByLabelText('连接名称'), ' renamed'); await user.click(screen.getByRole('button', { name: '保存' }));
    expect(vi.mocked(storageBridge.saveProfile).mock.calls[0][0].credential_update).toEqual({ kind: 'keep' }); expect(connect).not.toHaveBeenCalled(); expect(close).toHaveBeenCalledOnce();
  });
  it('blocks silent credential reuse for changed authentication target', async () => {
    const user = userEvent.setup(); render(<ConnectionDialog available size={size} profile={saved} status={status} onConnect={vi.fn()} onClose={vi.fn()} />);
    await user.type(screen.getByLabelText('主机地址'), '.other'); await user.click(screen.getByRole('button', { name: '保存' })); expect(storageBridge.saveProfile).not.toHaveBeenCalled(); expect(screen.getByRole('alert').textContent).toContain('认证目标已改变');
  });
  it('accepts unencrypted key without a made-up passphrase', async () => {
    const connect = vi.fn().mockResolvedValue(true); const user = userEvent.setup(); render(<ConnectionDialog available size={size} onConnect={connect} onClose={vi.fn()} />);
    await user.type(screen.getByLabelText('主机地址'), 'localhost'); await user.type(screen.getByLabelText('用户名'), 'tester'); await user.click(screen.getByRole('button', { name: '私钥文件' })); await user.type(screen.getByLabelText('本机私钥绝对路径'), '/tmp/synthetic_key'); await user.click(screen.getByRole('button', { name: '连接服务器' })); expect(connect.mock.calls[0][0].auth).toEqual({ kind: 'private_key', private_key_path: '/tmp/synthetic_key' });
  });
  it('blocks duplicate submissions synchronously and retains field validation', async () => {
    const connect = vi.fn(() => new Promise<boolean>(() => {})); const user = userEvent.setup(); render(<ConnectionDialog available size={size} onConnect={connect} onClose={vi.fn()} />); await fill(user);
    fireEvent.change(screen.getByLabelText('端口'), { target: { value: '99999' } }); await user.click(screen.getByRole('button', { name: '连接服务器' })); expect(connect).not.toHaveBeenCalled(); expect(screen.getByRole('alert').textContent).toContain('65535');
    fireEvent.change(screen.getByLabelText('端口'), { target: { value: '22' } }); const form = screen.getByLabelText('主机地址').closest('form')!; fireEvent.submit(form); fireEvent.submit(form); expect(connect).toHaveBeenCalledOnce();
  });
  it('saves metadata alone without requiring a password or causing network activity', async () => {
    const connect = vi.fn(); const user = userEvent.setup(); render(<ConnectionDialog available size={size} onConnect={connect} onClose={vi.fn()} />);
    await user.type(screen.getByLabelText('连接名称'), 'Offline'); await user.type(screen.getByLabelText('主机地址'), 'localhost'); await user.type(screen.getByLabelText('用户名'), 'tester'); await user.click(screen.getByLabelText('保存连接')); await user.click(screen.getByRole('button', { name: '仅保存连接' }));
    expect(storageBridge.saveProfile).toHaveBeenCalledOnce(); expect(connect).not.toHaveBeenCalled();
  });
  it('requires an explicit discard decision and clears cancelled inputs', async () => {
    const close = vi.fn(); const user = userEvent.setup(); render(<ConnectionDialog available size={size} onConnect={vi.fn()} onClose={close} />); await fill(user); await user.click(screen.getByRole('button', { name: '取消' })); expect(close).not.toHaveBeenCalled(); await user.click(screen.getByRole('button', { name: '放弃修改' })); expect(close).toHaveBeenCalledOnce(); expect(screen.queryByDisplayValue('SYNTHETIC_SECRET')).toBeNull();
  });
  it('never connects in browser preview', () => { render(<ConnectionDialog available={false} size={size} onConnect={vi.fn()} onClose={vi.fn()} />); expect((screen.getByRole('button', { name: '连接服务器' }) as HTMLButtonElement).disabled).toBe(true); });
});
