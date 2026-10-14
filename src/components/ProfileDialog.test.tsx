import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ProfileDialog } from './ProfileDialog';
import { storageBridge } from '../lib/storageBridge';
import type { ProfileStatus } from '../lib/storageBridge';
vi.mock('../lib/storageBridge', () => ({ storageBridge: { profiles: vi.fn(), saveProfile: vi.fn(), sessionCredential: vi.fn(), sessionMode: vi.fn() } }));
const status: ProfileStatus = { platform: 'linux', configuration_root: '/private/xyzaishell', credentials: { credential_backend: 'linux_secret_service', credential_availability: 'available', persistent_credentials: true }, listing: { revision: '7', mode: 'linux_secret_service', connections: [{ id: 'p1', label: 'Test', host: 'example.test', port: 22, username: 'tester', auth_kind: 'password', private_key_path: null, credential_present: true }] } };
describe('saved profiles and credential boundaries', () => {
  beforeEach(() => { vi.clearAllMocks(); vi.mocked(storageBridge.profiles).mockResolvedValue(status); vi.mocked(storageBridge.saveProfile).mockResolvedValue(status); });
  it('connects exact saved identity and revision without receiving stored secrets', async () => {
    const connect = vi.fn().mockResolvedValue(true); const user = userEvent.setup(); render(<ProfileDialog available onClose={vi.fn()} onConnect={connect} />);
    await user.selectOptions(await screen.findByLabelText('选择档案'), 'p1');
    expect(screen.queryByLabelText('档案密码')).toBeNull();
    await user.click(screen.getByRole('button', { name: '连接已保存档案' }));
    expect(connect).toHaveBeenCalledExactlyOnceWith('p1', '7'); expect(storageBridge.saveProfile).not.toHaveBeenCalled();
  });
  it('saves a secret only after explicit storage choice and clears its field', async () => {
    const user = userEvent.setup(); render(<ProfileDialog available onClose={vi.fn()} onConnect={vi.fn()} />);
    await user.selectOptions(await screen.findByLabelText('选择档案'), 'p1'); await user.selectOptions(screen.getByLabelText('凭据处理'), 'save');
    await user.type(screen.getByLabelText('档案密码'), 'PRIVATE_TEST_VALUE'); await user.click(screen.getByRole('button', { name: '保存档案' }));
    await waitFor(() => expect(storageBridge.saveProfile).toHaveBeenCalledOnce());
    expect(vi.mocked(storageBridge.saveProfile).mock.calls[0][0].credential_update).toEqual({ kind: 'set', value: 'PRIVATE_TEST_VALUE' });
    expect(screen.queryByDisplayValue('PRIVATE_TEST_VALUE')).toBeNull(); expect(localStorage.length).toBe(0); expect(storageBridge.sessionMode).not.toHaveBeenCalled();
  });
  it('blocks unsaved edited targets and ambiguous save replay until a read-only refresh', async () => {
    const user = userEvent.setup(); render(<ProfileDialog available onClose={vi.fn()} onConnect={vi.fn()} />);
    await user.selectOptions(await screen.findByLabelText('选择档案'), 'p1');
    await user.type(screen.getByLabelText('档案主机'), '.changed');
    expect((screen.getByRole('button', { name: '连接已保存档案' }) as HTMLButtonElement).disabled).toBe(true);
    vi.mocked(storageBridge.saveProfile).mockRejectedValueOnce(new Error('PRIVATE_ERROR_VALUE'));
    await user.click(screen.getByRole('button', { name: '保存档案' }));
    await waitFor(() => expect((screen.getByRole('button', { name: '保存档案' }) as HTMLButtonElement).disabled).toBe(true));
    expect(screen.queryByText('PRIVATE_ERROR_VALUE')).toBeNull();
    await user.click(screen.getByRole('button', { name: '重新读取档案状态' }));
    await waitFor(() => expect(storageBridge.profiles).toHaveBeenCalledTimes(2));
    expect(storageBridge.saveProfile).toHaveBeenCalledOnce();
  });
  it('keeps invalid local drafts editable without IPC, uncertainty or credential clearing', async () => {
    const user = userEvent.setup(); render(<ProfileDialog available onClose={vi.fn()} onConnect={vi.fn()} />);
    await screen.findByLabelText('选择档案'); await user.type(screen.getByLabelText('档案主机'), 'example.test');
    await user.type(screen.getByLabelText('档案用户名'), 'tester'); await user.selectOptions(screen.getByLabelText('凭据处理'), 'save'); await user.type(screen.getByLabelText('档案密码'), 'DRAFT_TEST_VALUE');
    await user.click(screen.getByRole('button', { name: '保存档案' }));
    expect(storageBridge.saveProfile).not.toHaveBeenCalled(); expect(screen.queryByText(/保存结果可能不确定/)).toBeNull();
    expect((screen.getByRole('button', { name: '保存档案' }) as HTMLButtonElement).disabled).toBe(false);
    expect((screen.getByLabelText('档案密码') as HTMLInputElement).value).toBe('DRAFT_TEST_VALUE');
    await user.type(screen.getByLabelText('档案名称'), 'Corrected draft'); await user.click(screen.getByRole('button', { name: '保存档案' }));
    expect(storageBridge.saveProfile).toHaveBeenCalledOnce(); expect(storageBridge.profiles).toHaveBeenCalledOnce();
  });
  it('binds a session credential to the revision returned by its profile save', async () => {
    const sessionStatus: ProfileStatus = { ...status, credentials: { credential_backend: 'session_only', credential_availability: 'available', persistent_credentials: false }, listing: { ...status.listing, mode: 'session_only' } };
    const savedStatus = { ...sessionStatus, listing: { ...sessionStatus.listing, revision: '8' } };
    vi.mocked(storageBridge.profiles).mockResolvedValue(sessionStatus); vi.mocked(storageBridge.saveProfile).mockResolvedValue(savedStatus); vi.mocked(storageBridge.sessionCredential).mockResolvedValue(savedStatus);
    const user = userEvent.setup(); render(<ProfileDialog available onClose={vi.fn()} onConnect={vi.fn()} />);
    await user.selectOptions(await screen.findByLabelText('选择档案'), 'p1'); await user.selectOptions(screen.getByLabelText('凭据处理'), 'session'); await user.type(screen.getByLabelText('档案密码'), 'SESSION_TEST_VALUE'); await user.click(screen.getByRole('button', { name: '保存档案' }));
    expect(storageBridge.sessionCredential).toHaveBeenCalledExactlyOnceWith('p1', '8', 'SESSION_TEST_VALUE');
  });
  it('does not silently downgrade unavailable storage', async () => {
    vi.mocked(storageBridge.profiles).mockResolvedValue({ ...status, credentials: { ...status.credentials, credential_availability: 'unavailable', persistent_credentials: false } });
    const user = userEvent.setup(); render(<ProfileDialog available onClose={vi.fn()} onConnect={vi.fn()} />);
    const button = await screen.findByRole('button', { name: '确认切换全部档案' }); expect((button as HTMLButtonElement).disabled).toBe(true); expect(storageBridge.sessionMode).not.toHaveBeenCalled();
    vi.mocked(storageBridge.sessionMode).mockResolvedValue({ ...status, listing: { ...status.listing, mode: 'session_only' } });
    await user.click(screen.getByRole('checkbox')); await user.click(button); expect(storageBridge.sessionMode).toHaveBeenCalledExactlyOnceWith('7');
  });
});
