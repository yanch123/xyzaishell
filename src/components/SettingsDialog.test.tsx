import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ModelConfigStatus, ModelEvent, ModelTestResult, SaveModelConfigRequest } from '../lib/model';
import { modelConfig, chat } from '../test/modelFixtures';
const mocks = vi.hoisted(() => ({ native: vi.fn(), listen: vi.fn(), config: vi.fn(), list: vi.fn(), create: vi.fn(), get: vi.fn(), send: vi.fn(), cancel: vi.fn(), reload: vi.fn(), confirm: vi.fn(), probe: vi.fn(), save: vi.fn(), testSaved: vi.fn(), cancelTest: vi.fn() }));
vi.mock('../lib/bridge', () => ({ desktopAvailable: mocks.native }));
vi.mock('../lib/modelBridge', () => ({ modelBridge: mocks }));
vi.mock('./HistorySettings', () => ({ HistorySettings: () => <p>Existing history controls</p> }));
import { useModelChat } from '../hooks/useModelChat';
import { SettingsDialog } from './SettingsDialog';
let emit: (event: ModelEvent) => void;
const close = vi.fn(); const consent = vi.fn();
let revision: number;
let saved: ModelConfigStatus;
function Harness() { const model = useModelChat(); return <><button type="button">终端背景仍可操作</button><SettingsDialog model={model} onClose={close} onConsent={consent} /></>; }
beforeEach(() => {
  vi.resetAllMocks(); revision = 1; saved = { ...modelConfig };
  mocks.native.mockReturnValue(true); mocks.listen.mockImplementation(async handler => { emit = handler; return vi.fn(); });
  mocks.config.mockResolvedValue(saved); mocks.list.mockResolvedValue([chat()]); mocks.reload.mockImplementation(async () => saved);
  mocks.save.mockImplementation(async (request: SaveModelConfigRequest) => {
    saved = { ...saved, revision: String(++revision), file_revision: `saved-${revision}`, base_url: request.base_url.replace(/\/+$/, ''), destination: request.base_url.replace(/\/+$/, ''), model: request.model, consented: false, key_present: request.key_update.kind !== 'clear', ready: request.key_update.kind !== 'clear' };
    return saved;
  });
  mocks.testSaved.mockImplementation(async request => ({ ...request, success: true, error: null }));
});
async function ready() { render(<Harness />); await waitFor(() => expect((screen.getByLabelText(/^Base URL/) as HTMLInputElement).value).toBe(modelConfig.base_url)); return userEvent.setup(); }
const button = (name: string) => screen.getByRole('button', { name });
describe('v0.1.2 saved model settings', () => {
  it('is nonmodal, keeps navigation and never reads an old key into the form', async () => {
    const user = await ready();
    expect(screen.getByRole('dialog').getAttribute('aria-modal')).toBe('false');
    expect((screen.getByLabelText(/^API Key/) as HTMLInputElement).value).toBe('');
    expect((screen.getByLabelText(/^API Key/) as HTMLInputElement).type).toBe('password');
    await user.click(button('通用')); expect(screen.getByText('能力与边界')).toBeDefined();
    await user.click(button('历史记录')); expect(screen.getByText('Existing history controls')).toBeDefined();
    await user.click(button('终端背景仍可操作')); expect(document.activeElement).toBe(button('终端背景仍可操作'));
    expect(mocks.save).not.toHaveBeenCalled(); expect(mocks.testSaved).not.toHaveBeenCalled();
  });
  it('saves keep intent and expected file version without network or old-key roundtrip', async () => {
    const user = await ready(); await user.clear(screen.getByLabelText(/^模型名/)); await user.type(screen.getByLabelText(/^模型名/), 'next-model'); await user.click(button('保存'));
    expect(mocks.save).toHaveBeenCalledExactlyOnceWith({ expected_file_revision: 'saved-file-1', base_url: modelConfig.base_url, model: 'next-model', key_update: { kind: 'keep' }, reuse_key_for_destination: null });
    expect(mocks.testSaved).not.toHaveBeenCalled(); expect(screen.getByText(/模型设置已保存。发送聊天前/)).toBeDefined();
  });
  it('requires explicit destination-bound reuse, or a newly entered key', async () => {
    const user = await ready(); await user.clear(screen.getByLabelText(/^Base URL/)); await user.type(screen.getByLabelText(/^Base URL/), 'https://new.example/v1/'); await user.click(button('保存'));
    expect(mocks.save).not.toHaveBeenCalled();
    await user.click(screen.getByRole('checkbox', { name: /将已有 Key 用于这个新目标/ })); await user.click(button('保存'));
    expect(mocks.save.mock.calls[0][0].reuse_key_for_destination).toBe('https://new.example/v1');
    await user.type(screen.getByLabelText(/^API Key/), 'synthetic-replacement'); await user.click(button('保存'));
    expect(mocks.save.mock.calls[1][0].key_update).toEqual({ kind: 'replace', value: 'synthetic-replacement' });
    expect((screen.getByLabelText(/^API Key/) as HTMLInputElement).value).toBe('');
  });
  it('clears a key only through its separate explicit decision and save', async () => {
    const user = await ready(); await user.click(button('清除已保存 Key…')); expect(mocks.save).not.toHaveBeenCalled();
    await user.click(button('确认清除，保存时生效')); expect(mocks.save).not.toHaveBeenCalled();
    await user.click(button('保存')); expect(mocks.save.mock.calls[0][0].key_update).toEqual({ kind: 'clear' });
    expect(screen.getByText(/尚未配置 API Key，发送已禁用/)).toBeDefined(); expect(screen.queryByRole('button', { name: '测试已保存配置' })).toBeNull();
  });
  it('save-and-test saves first and tests only the confirmed returned snapshot', async () => {
    const user = await ready(); await user.click(button('保存并测试'));
    expect(mocks.save).toHaveBeenCalledOnce(); expect(mocks.testSaved).not.toHaveBeenCalled();
    const decision = screen.getByRole('group', { name: '确认测试接收方' });
    expect(decision.textContent).toContain('Reply with OK.'); expect(decision.textContent).toContain('不包含聊天历史');
    await user.click(within(decision).getByRole('checkbox')); await user.click(button('确认并测试'));
    expect(mocks.testSaved).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ file_revision: 'saved-2', destination: saved.destination, model: saved.model, consent: true }));
    expect(mocks.confirm).not.toHaveBeenCalled(); expect(mocks.probe).not.toHaveBeenCalled(); expect(mocks.send).not.toHaveBeenCalled();
    expect(screen.getByText('流式聊天测试通过')).toBeDefined();
  });
  it('separates saved state from a failed test, and never automatically retries', async () => {
    mocks.testSaved.mockImplementation(async request => ({ ...request, success: false, error: { code: 'MODEL_AUTH_FAILED', safe_message: 'Credentials rejected.', retry: 'after_user_action' } }));
    const user = await ready(); await user.click(button('保存并测试')); await user.click(screen.getByRole('checkbox', { name: /同意向这个已保存目标/ })); await user.click(button('确认并测试'));
    expect(screen.getByText('设置已保存，测试失败。')).toBeDefined(); expect(screen.getByText('Credentials rejected.')).toBeDefined();
    expect(mocks.save).toHaveBeenCalledOnce(); expect(mocks.testSaved).toHaveBeenCalledOnce();
  });
  it('never tests after an uncertain save and hides raw errors', async () => {
    mocks.save.mockRejectedValue({ code: 'CONFIG_SAVE_UNCERTAIN', safe_message: 'Reload before saving again.', retry: 'after_user_action' });
    const user = await ready(); await user.click(button('保存并测试'));
    expect(mocks.testSaved).not.toHaveBeenCalled(); expect(screen.queryByRole('group', { name: '确认测试接收方' })).toBeNull(); expect(screen.getByText('Reload before saving again.')).toBeDefined(); expect((button('保存') as HTMLButtonElement).disabled).toBe(true); expect((button('测试已保存配置') as HTMLButtonElement).disabled).toBe(true);
  });
  it('keeps a draft when an external config event arrives and requires reload', async () => {
    const user = await ready(); await user.type(screen.getByLabelText(/^模型名/), '-draft');
    act(() => emit({ kind: 'config_updated', config: { ...modelConfig, revision: '9', file_revision: 'external-9', model: 'external-model' } }));
    expect((screen.getByLabelText(/^模型名/) as HTMLInputElement).value).toBe('test-model-draft');
    expect((button('保存') as HTMLButtonElement).disabled).toBe(true);
    await user.click(button('重新加载模型配置')); expect(screen.getByRole('group', { name: '处理未保存修改' })).toBeDefined(); expect(mocks.reload).not.toHaveBeenCalled();
  });
  it('offers save/discard/cancel on closing a dirty form', async () => {
    const user = await ready(); await user.type(screen.getByLabelText(/^模型名/), '-draft'); await user.click(button('关闭设置')); expect(close).not.toHaveBeenCalled();
    await user.click(button('继续编辑')); expect(close).not.toHaveBeenCalled();
    await user.click(button('关闭设置')); await user.click(button('放弃修改并关闭')); expect(close).toHaveBeenCalledOnce(); expect(mocks.save).not.toHaveBeenCalled();
  });
  it('marks an earlier successful test as stale after editing and can cancel a running test', async () => {
    const user = await ready(); await user.click(button('测试已保存配置')); await user.click(screen.getByRole('checkbox', { name: /同意向这个已保存目标/ })); await user.click(button('确认并测试'));
    await user.type(screen.getByLabelText(/^模型名/), '-draft'); expect(screen.getByText(/不代表当前草稿或新配置已通过/)).toBeDefined();
    let finish!: (result: ModelTestResult) => void; mocks.testSaved.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
    await user.click(button('保存并测试')); await user.click(screen.getByRole('checkbox', { name: /同意向这个已保存目标/ })); await user.click(button('确认并测试')); await user.click(button('取消测试'));
    expect(mocks.cancelTest).toHaveBeenCalledExactlyOnceWith(mocks.testSaved.mock.calls[1][0].test_id);
    await act(async () => finish({ ...mocks.testSaved.mock.calls[1][0], success: false, error: { code: 'MODEL_CANCELLED', safe_message: 'Cancelled.', retry: 'never' } }));
    expect(screen.getByText('设置已保存，测试已取消。')).toBeDefined();
  });
  it('validates endpoint paths and model limits before IPC', async () => {
    const user = await ready(); fireEvent.change(screen.getByLabelText(/^Base URL/), { target: { value: 'https://example.com/v1/chat/completions' } }); await user.click(button('保存'));
    expect(mocks.save).not.toHaveBeenCalled(); expect(screen.getByText(/请填写有效 HTTPS API 根地址/)).toBeDefined();
  });
});
