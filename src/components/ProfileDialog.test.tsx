import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ProfileDialog } from './ProfileDialog';
import { storageBridge } from '../lib/storageBridge';
import type { ProfileStatus } from '../lib/storageBridge';
vi.mock('../lib/storageBridge', () => ({ storageBridge: { profiles: vi.fn(), saveProfile: vi.fn(), deleteProfile: vi.fn() } }));
const status: ProfileStatus = { platform: 'linux', configuration_root: '/private', credentials: { credential_backend: 'linux_secret_service', credential_availability: 'available', persistent_credentials: true }, listing: { revision: '7', mode: 'linux_secret_service', connections: [{ id: 'p1', label: 'Test', host: 'example.test', port: 22, username: 'tester', auth_kind: 'password', private_key_path: null, credential_present: true, credential_policy: 'system_secure' }] } };
describe('saved connections and scoped credential challenges', () => {
  beforeEach(() => { vi.clearAllMocks(); vi.mocked(storageBridge.profiles).mockResolvedValue(status); vi.mocked(storageBridge.saveProfile).mockResolvedValue(status); });
  it('connects a fresh saved revision without exposing secrets', async () => {
    const connect = vi.fn().mockResolvedValue(true); const user = userEvent.setup(); render(<ProfileDialog available onClose={vi.fn()} onConnect={connect} initialProfileId="p1" />); await screen.findByLabelText('选择档案'); await user.click(screen.getByRole('button', { name: '连接已保存档案' }));
    expect(connect).toHaveBeenCalledExactlyOnceWith('p1', '7', undefined); expect(storageBridge.profiles).toHaveBeenCalledTimes(2); expect(screen.queryByLabelText('本次密码')).toBeNull(); expect(storageBridge.saveProfile).not.toHaveBeenCalled();
  });
  it('asks for missing password at connection time and cancel never authenticates', async () => {
    vi.mocked(storageBridge.profiles).mockResolvedValue({ ...status, listing: { ...status.listing, connections: [{ ...status.listing.connections[0], credential_policy: 'ask_on_connect', credential_present: false }] } });
    const connect = vi.fn(); const user = userEvent.setup(); render(<ProfileDialog available onClose={vi.fn()} onConnect={connect} initialProfileId="p1" />); await screen.findByLabelText('选择档案'); await user.click(screen.getByRole('button', { name: '连接已保存档案' })); await user.type(screen.getByLabelText('本次密码'), 'SYNTHETIC_ONLY_ONCE'); await user.click(screen.getByRole('button', { name: '取消认证' })); expect(connect).not.toHaveBeenCalled(); expect(screen.queryByDisplayValue('SYNTHETIC_ONLY_ONCE')).toBeNull();
  });
  it('uses a temporary password despite unavailable provider without downgrading profiles', async () => {
    vi.mocked(storageBridge.profiles).mockResolvedValue({ ...status, credentials: { ...status.credentials, credential_availability: 'locked' } });
    const connect = vi.fn().mockResolvedValue(false); const user = userEvent.setup(); render(<ProfileDialog available onClose={vi.fn()} onConnect={connect} initialProfileId="p1" />); await screen.findByLabelText('选择档案'); await user.click(screen.getByRole('button', { name: '连接已保存档案' })); await user.type(screen.getByLabelText('本次密码'), 'SYNTHETIC_ONLY_ONCE'); await user.click(screen.getByRole('button', { name: '仅本次输入连接' }));
    expect(connect).toHaveBeenCalledExactlyOnceWith('p1', '7', { temporaryCredential: 'SYNTHETIC_ONLY_ONCE', rememberCredential: false }); expect(storageBridge.saveProfile).not.toHaveBeenCalled(); expect(screen.queryByDisplayValue('SYNTHETIC_ONLY_ONCE')).toBeNull(); expect(localStorage.length).toBe(0);
  });
  it('prompts only after backend reports an encrypted private key needs its passphrase', async () => {
    vi.mocked(storageBridge.profiles).mockResolvedValue({ ...status, listing: { ...status.listing, connections: [{ ...status.listing.connections[0], auth_kind: 'private_key', private_key_path: '/synthetic/key', credential_present: false, credential_policy: 'ask_on_connect' }] } });
    const connect = vi.fn().mockRejectedValueOnce({ code: 'CREDENTIAL_PASSPHRASE_REQUIRED', safe_message: '此私钥需要口令。' }); const user = userEvent.setup(); render(<ProfileDialog available onClose={vi.fn()} onConnect={connect} initialProfileId="p1" />); await screen.findByLabelText('选择档案'); expect(screen.queryByLabelText('本次私钥口令')).toBeNull(); await user.click(screen.getByRole('button', { name: '连接已保存档案' })); expect(await screen.findByLabelText('本次私钥口令')).toBeDefined(); expect(connect).toHaveBeenCalledOnce();
  });
  it('confirms saving a challenged secret before a separate saved-profile connect', async () => {
    const missing: ProfileStatus = { ...status, listing: { ...status.listing, connections: [{ ...status.listing.connections[0], credential_present: false, credential_policy: 'ask_on_connect' }] } };
    vi.mocked(storageBridge.profiles).mockResolvedValue(missing); vi.mocked(storageBridge.saveProfile).mockResolvedValue({ ...status, listing: { ...status.listing, revision: '8' } });
    const connect = vi.fn().mockResolvedValue(false); const user = userEvent.setup(); render(<ProfileDialog available onClose={vi.fn()} onConnect={connect} initialProfileId="p1" />); await screen.findByLabelText('选择档案'); await user.click(screen.getByRole('button', { name: '连接已保存档案' })); await user.type(screen.getByLabelText('本次密码'), 'SYNTHETIC_REMEMBER'); await user.click(screen.getByRole('checkbox', { name: /为此连接记住凭据/ })); await user.click(screen.getByRole('button', { name: '保存凭据并连接' }));
    expect(vi.mocked(storageBridge.saveProfile).mock.calls[0][0].credential_update).toEqual({ kind: 'set', value: 'SYNTHETIC_REMEMBER' }); expect(connect).toHaveBeenCalledExactlyOnceWith('p1', '8', undefined); expect(screen.getByRole('status').textContent).toContain('凭据已保存'); expect(screen.queryByDisplayValue('SYNTHETIC_REMEMBER')).toBeNull();
  });
  it('opens the same editor for metadata changes, save never connects', async () => {
    const connect = vi.fn(); const user = userEvent.setup(); render(<ProfileDialog available onClose={vi.fn()} onConnect={connect} initialProfileId="p1" />); await screen.findByLabelText('选择档案'); await user.click(screen.getByRole('button', { name: '编辑 / 重命名' })); await user.type(screen.getByLabelText('连接名称'), ' renamed'); await user.click(screen.getByRole('button', { name: '保存' }));
    await waitFor(() => expect(storageBridge.saveProfile).toHaveBeenCalledOnce()); expect(connect).not.toHaveBeenCalled(); expect(vi.mocked(storageBridge.saveProfile).mock.calls[0][0].credential_update).toEqual({ kind: 'keep' });
  });
  it('launches active-session alternatives before asking for secrets', async () => {
    const launch = vi.fn(); const connect = vi.fn(); const user = userEvent.setup(); render(<ProfileDialog available activeSession onConnectInNewWindow={launch} onClose={vi.fn()} onConnect={connect} initialProfileId="p1" />); await screen.findByLabelText('选择档案'); await user.click(screen.getByRole('button', { name: '在新窗口连接' })); expect(launch).toHaveBeenCalledExactlyOnceWith('p1'); expect(connect).not.toHaveBeenCalled(); expect(screen.queryByLabelText('本次密码')).toBeNull();
  });
  it('requires a scoped deletion confirmation and retains other records', async () => {
    vi.mocked(storageBridge.deleteProfile).mockResolvedValue({ ...status, listing: { ...status.listing, revision: '8', connections: [] } });
    const user = userEvent.setup(); render(<ProfileDialog available onClose={vi.fn()} onConnect={vi.fn()} initialProfileId="p1" />); await screen.findByLabelText('选择档案'); await user.click(screen.getByRole('button', { name: '删除连接' })); expect(storageBridge.deleteProfile).not.toHaveBeenCalled(); await user.click(screen.getByRole('button', { name: '确认删除此连接' })); expect(storageBridge.deleteProfile).toHaveBeenCalledExactlyOnceWith('p1', '7');
  });
});
