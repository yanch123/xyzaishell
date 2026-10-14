import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { TaskSecretPanel } from './TaskSecretPanel';
import { taskSecretsBridge } from '../lib/storageBridge';
import { controller, task } from '../test/agentFixtures';
vi.mock('../lib/storageBridge', () => ({ taskSecretsBridge: { get: vi.fn(), ensure: vi.fn(), reload: vi.fn() } }));
const view = { task_id: 'chat-a', snapshot_id: 'snapshot-a', fields: [{ name: 'service_password', present: true }], local_edit_path: '/private/tasks/chat-a.toml' };
describe('task-local secrets without value exposure', () => {
  beforeEach(() => { vi.clearAllMocks(); vi.mocked(taskSecretsBridge.get).mockResolvedValue(view); vi.mocked(taskSecretsBridge.reload).mockResolvedValue(view); });
  it('requires exact field, purpose and prompt confirmation before preparing an action', async () => {
    const prepare = vi.fn(); const user = userEvent.setup(); const current = task({ phase: 'needs_secrets', secret_request: { fields: [{ name: 'service_password', required: true }], purpose: 'Authenticate service' } });
    render(<TaskSecretPanel task={current} agent={controller({ prepareSecret: prepare })} connected />);
    await screen.findByText(/本机编辑路径/); expect(taskSecretsBridge.get).toHaveBeenCalledExactlyOnceWith('chat-a', 'run-a');
    const button = screen.getByRole('button', { name: '核对本次秘密输入动作' }); expect((button as HTMLButtonElement).disabled).toBe(true);
    await user.selectOptions(screen.getByLabelText('本次输入秘密字段'), 'service_password'); expect((button as HTMLButtonElement).disabled).toBe(true);
    await user.click(screen.getByRole('checkbox')); await user.click(button);
    expect(prepare).toHaveBeenCalledExactlyOnceWith('chat-a', 'run-a', 'service_password', 'Authenticate service');
    expect((button as HTMLButtonElement).disabled).toBe(true); expect(screen.queryByLabelText(/密码值/)).toBeNull(); expect(localStorage.length).toBe(0);
  });
  it('clears prompt consent when reload changes secret snapshot and blocks disconnected input', async () => {
    const user = userEvent.setup(); const current = task({ phase: 'needs_secrets', secret_request: { fields: [{ name: 'service_password', required: true }], purpose: 'Auth' } });
    const agent = controller({ prepareSecret: vi.fn() }); const rendered = render(<TaskSecretPanel task={current} agent={agent} connected />);
    await screen.findByLabelText('本次输入秘密字段'); await user.selectOptions(screen.getByLabelText('本次输入秘密字段'), 'service_password'); await user.click(screen.getByRole('checkbox'));
    await user.click(screen.getByRole('button', { name: '重新加载已填写状态' })); await waitFor(() => expect((screen.getByRole('checkbox') as HTMLInputElement).checked).toBe(false));
    rendered.rerender(<TaskSecretPanel task={current} agent={agent} connected={false} />); expect((screen.getByRole('checkbox') as HTMLInputElement).disabled).toBe(true); expect(agent.prepareSecret).not.toHaveBeenCalled();
  });
});
