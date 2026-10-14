import { test, expect } from '@playwright/test';

test('synthetic storage: unified profile editor, explicit credential policy, history consent and inactive restore', async ({ page }, testInfo) => {
  await page.addInitScript(() => {
    const calls: { command: string; args: unknown }[] = [];
    const callbacks = new Map<number, unknown>(); let next = 1;
    const root = window as unknown as Record<string, unknown>;
    const profile = { platform: 'linux', configuration_root: '/synthetic/private', credentials: { credential_backend: 'linux_secret_service', credential_availability: 'available', persistent_credentials: true }, listing: { revision: '1', mode: 'linux_secret_service', connections: [{ id: 'saved-profile', label: 'Synthetic profile', host: 'example.invalid', port: 22, username: 'test', auth_kind: 'password', private_key_path: null, credential_present: true, credential_policy: 'system_secure' }] as Array<Record<string, unknown>> } };
    const config = { path: '/synthetic/model.toml', revision: '1', file_revision: 'synthetic-model-file-v1', base_url: null, key_present: false, path_overridden: false, ready: false, destination: null, model: null, consented: false, error: null, warning: null, context_policy: 'explicit_chat_only', persistence: 'memory_only' };
    const history = { enabled: false, manageable: false, available: true, plaintext: true, revision: '1', error: null };
    let archived = true;
    const initial = { chat_id: 'current', task_id: 'current', title: '新任务', messages: [], active_run_id: null, revision: '1' };
    root.isTauri = true; root.__M4_CALLS__ = calls;
    root.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    root.__TAURI_INTERNALS__ = {
      transformCallback: (callback: unknown) => { const id = next++; callbacks.set(id, callback); return id; }, unregisterCallback: (id: number) => callbacks.delete(id),
      invoke: async (command: string, args: Record<string, unknown>) => {
        calls.push({ command, args });
        if (command === 'plugin:event|listen') return args.handler;
        if (command === 'plugin:event|unlisten') return;
        if (command === 'get_session_snapshot') return { session: null, state: 'disconnected', revision: '1' };
        if (command === 'get_window_launch_target') return { profile_id: null, new_connection: false };
        if (command === 'get_model_config_status') return config;
        if (command === 'list_chats') return [initial];
        if (command === 'list_terminal_tasks') return [];
        if (command === 'get_profile_status') return structuredClone(profile);
        if (command === 'save_connection_profile') {
          const request = args.request as { profile: Record<string, unknown>; credential_update: { kind: string; value?: string } };
          if (request.credential_update.kind !== 'set') throw new Error('Explicit credential storage required');
          profile.listing.revision = '2'; profile.listing.connections = [{ ...request.profile, id: 'saved-profile', credential_present: true, credential_policy: 'system_secure' }]; return structuredClone(profile);
        }
        if (command === 'get_shell_log_status') return { enabled: false, available: true, plaintext: true, revision: '1', state: 'disabled', session: null, reason: null, error: null };
        if (command === 'query_shell_log') return { entries: [], next_before_seq: null, snapshot_max_seq: '0' };
        if (command === 'get_history_status') return structuredClone(history);
        if (command === 'set_history_enabled') { history.enabled = Boolean(args.enabled); history.manageable = history.enabled; return structuredClone(history); }
        if (command === 'list_saved_chats') return archived ? [{ chat_id: 'restored', task_id: 'stable-task', title: '已保存的只读文字', message_count: 1, revision: '4', updated_ms: 1, listed: true }] : [];
        if (command === 'forget_saved_chat') { archived = false; return structuredClone(history); }
        if (command === 'restore_saved_chat') return { chat_id: 'restored', task_id: 'stable-task', title: '已保存的只读文字', restored_read_only: true, source: null, active_run_id: null, revision: '5', messages: [{ id: 'm1', role: 'assistant', text: '已中断的历史内容，不会重放。', state: 'cancelled', run_id: 'old-run', error: null }] };
        throw new Error(`Unexpected test IPC: ${command}`);
      },
    };
  });
  await page.goto('/');
  await page.getByRole('navigation', { name: '应用菜单' }).getByText('查看', { exact: true }).click();
  await page.screenshot({ path: testInfo.outputPath('workspace-main-menu.png'), fullPage: true });
  await page.getByRole('navigation', { name: '应用菜单' }).getByText('查看', { exact: true }).click();
  await page.getByRole('button', { name: '打开连接管理', exact: true }).click();
  const profiles = page.getByRole('dialog', { name: '已保存的 SSH 连接' });
  await expect(profiles).toHaveAttribute('aria-modal', 'false');
  await profiles.getByLabel('选择档案').selectOption('saved-profile');
  await profiles.getByRole('button', { name: '编辑 / 重命名', exact: true }).click();
  const editor = page.getByRole('dialog', { name: '编辑 SSH 连接' });
  await expect(editor.getByLabel('登录密码')).toHaveValue('');
  await editor.getByLabel('连接名称', { exact: true }).fill('Synthetic renamed profile');
  await editor.getByLabel('登录密码').fill('SYNTHETIC_TEST_ONLY');
  await editor.getByRole('button', { name: '保存', exact: true }).click();
  await expect(editor).toBeHidden();
  await expect(profiles.getByLabel('选择档案')).toHaveValue('saved-profile');
  await expect(profiles.getByLabel('选择档案')).toContainText('Synthetic renamed profile');
  await expect(page.locator('input[type=password]')).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath('storage-profile.png'), fullPage: true });
  await profiles.getByRole('button', { name: '关闭', exact: true }).click();
  await page.getByRole('button', { name: '打开设置', exact: true }).click();
  const settings = page.getByRole('dialog', { name: '工作空间设置' });
  await page.screenshot({ path: testInfo.outputPath('workspace-model-settings.png'), fullPage: true });
  await settings.getByRole('button', { name: '历史记录', exact: true }).click();
  await expect(settings.getByRole('button', { name: '启用明文聊天历史' })).toBeDisabled();
  await settings.getByRole('checkbox', { name: /我理解聊天将以明文/ }).check(); await settings.getByRole('button', { name: '启用明文聊天历史' }).click();
  await settings.getByRole('button', { name: '仅恢复文字' }).click();
  await expect(page.getByText('已中断的历史内容，不会重放。')).toBeVisible();
  const calls = await page.evaluate(() => (window as unknown as { __M4_CALLS__: { command: string; args: unknown }[] }).__M4_CALLS__);
  expect(calls.filter(item => item.command === 'save_connection_profile')).toHaveLength(1);
  expect(calls.filter(item => ['connect_server', 'connect_saved_profile', 'send_chat_message', 'start_terminal_task', 'send_user_input', 'decide_terminal_action'].includes(item.command))).toHaveLength(0);
  expect(await page.evaluate(() => localStorage.length)).toBe(0);
  await page.screenshot({ path: testInfo.outputPath('m4-restored-chat.png'), fullPage: true });
  await page.getByRole('button', { name: '打开设置', exact: true }).click();
  await settings.getByRole('button', { name: '历史记录', exact: true }).click();
  await settings.getByRole('button', { name: '移除历史：已保存的只读文字' }).click();
  await expect(settings.getByRole('group', { name: '确认移除单条历史' })).toContainText('restored');
  await settings.getByRole('button', { name: '取消移除' }).click();
  expect(await page.evaluate(() => (window as unknown as { __M4_CALLS__: { command: string }[] }).__M4_CALLS__.filter(item => item.command === 'forget_saved_chat').length)).toBe(0);
  await settings.getByRole('button', { name: '移除历史：已保存的只读文字' }).click();
  await settings.getByRole('button', { name: '确认移除此条历史' }).click();
  await expect(settings.getByText('尚无可恢复的聊天。')).toBeVisible();
  const forgotten = await page.evaluate(() => (window as unknown as { __M4_CALLS__: { command: string; args: unknown }[] }).__M4_CALLS__.filter(item => item.command === 'forget_saved_chat'));
  expect(forgotten).toEqual([{ command: 'forget_saved_chat', args: { chatId: 'restored', expectedRevision: '4' } }]);

  await settings.getByRole('button', { name: '关闭设置', exact: true }).click();
  await page.getByRole('button', { name: '打开终端日志', exact: true }).click();
  const logs = page.getByRole('dialog', { name: '终端日志' });
  await expect(logs).toHaveAttribute('aria-modal', 'false');
  await logs.getByLabel('日志查看范围').selectOption('all');
  await expect(logs.getByText('当前筛选下暂无日志。')).toBeVisible();
  await expect(logs.getByRole('checkbox')).toHaveCount(1);
  await expect(logs.getByRole('checkbox')).toBeDisabled();
  await expect(logs.getByRole('button', { name: '启动终端日志' })).toBeDisabled();
  await page.screenshot({ path: testInfo.outputPath('workspace-offline-operation-logs.png'), fullPage: true });
  await logs.getByRole('button', { name: '关闭对话框', exact: true }).click();
  expect(await page.evaluate(() => (window as unknown as { __M4_CALLS__: { command: string }[] }).__M4_CALLS__.some(item => item.command === 'start_shell_log'))).toBe(false);

});
