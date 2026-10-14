import { test, expect } from '@playwright/test';

test('synthetic settings: save, minimal-test consent, key non-echo and unsaved close', async ({ page }, testInfo) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => {
    const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
    const root = window as unknown as Record<string, unknown>;
    const callbacks = new Map<number, unknown>(); let next = 1;
    const config = { path: '/synthetic/private/model.toml', revision: '1', file_revision: 'file-1', base_url: null as string | null, key_present: false, path_overridden: false, ready: false, destination: null as string | null, model: null as string | null, consented: false, error: null, warning: null, context_policy: 'explicit_chat_only', persistence: 'memory_only' };
    root.isTauri = true; root.__SETTINGS_CALLS__ = calls;
    root.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    root.__TAURI_INTERNALS__ = {
      transformCallback: (callback: unknown) => { const id = next++; callbacks.set(id, callback); return id; },
      unregisterCallback: (id: number) => callbacks.delete(id),
      invoke: async (command: string, args: Record<string, unknown>) => {
        calls.push({ command, args });
        if (command === 'plugin:event|listen') return args.handler;
        if (command === 'plugin:event|unlisten') return;
        if (command === 'get_session_snapshot') return { state: 'disconnected', session: null, revision: '1' };
        if (command === 'get_window_launch_target') return { profile_id: null, new_connection: false };
        if (command === 'get_model_config_status') return structuredClone(config);
        if (command === 'list_chats') return [{ chat_id: 'fixture-chat', task_id: 'fixture-chat', title: '新任务', revision: '1', messages: [], active_run_id: null }];
        if (command === 'list_terminal_tasks') return [];
        if (command === 'get_history_status') return { enabled: false, manageable: false, available: true, plaintext: true, revision: '1', error: null };
        if (command === 'save_model_config') {
          const request = args.request as { expected_file_revision: string; base_url: string; model: string; key_update: { kind: string; value?: string } };
          if (request.expected_file_revision !== config.file_revision || request.key_update.kind !== 'replace' || request.key_update.value !== 'SYNTHETIC_BROWSER_KEY') throw new Error('Unexpected settings save');
          Object.assign(config, { revision: '2', file_revision: 'file-2', base_url: request.base_url, destination: request.base_url, model: request.model, key_present: true, ready: true, consented: false });
          return structuredClone(config);
        }
        if (command === 'test_model_config') {
          const request = args.request as { test_id: string; file_revision: string; destination: string; model: string; consent: boolean };
          if (request.file_revision !== config.file_revision || request.destination !== config.destination || request.model !== config.model || !request.consent) throw new Error('Unexpected test identity');
          return { ...request, success: true, error: null };
        }
        throw new Error(`Unexpected settings IPC: ${command}`);
      },
    };
  });
  await page.goto('/');
  await page.getByRole('button', { name: '打开设置', exact: true }).click();
  const settings = page.getByRole('dialog', { name: '工作空间设置' });
  await expect(settings).toHaveAttribute('aria-modal', 'false');
  await expect(settings.getByRole('button', { name: '保存', exact: true })).toBeEnabled();
  await settings.getByLabel('Base URL').fill('https://model-fixture.example/v1');
  await settings.getByLabel('API Key', { exact: false }).fill('SYNTHETIC_BROWSER_KEY');
  await settings.getByLabel('模型名').fill('synthetic-model');
  await settings.getByRole('button', { name: '保存并测试', exact: true }).click();
  const consent = settings.getByRole('group', { name: '确认测试接收方' });
  await expect(consent).toBeVisible();
  await expect(consent.getByRole('button', { name: '确认并测试', exact: true })).toBeDisabled();
  await expect(settings.getByLabel('API Key', { exact: false })).toHaveValue('');
  expect(await page.evaluate(() => (window as unknown as { __SETTINGS_CALLS__: { command: string }[] }).__SETTINGS_CALLS__.filter(item => item.command === 'test_model_config'))).toHaveLength(0);
  await page.screenshot({ path: testInfo.outputPath('settings-saved-awaiting-test-consent.png'), fullPage: true });
  await consent.getByRole('checkbox').check();
  await consent.getByRole('button', { name: '确认并测试', exact: true }).click();
  await expect(settings.getByText('设置已保存，流式聊天测试通过。未验证工具调用。')).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('settings-minimal-test-complete.png'), fullPage: true });
  await settings.getByLabel('模型名').fill('unsaved-model');
  await settings.getByRole('button', { name: '关闭设置', exact: true }).click();
  const discard = settings.getByRole('group', { name: '处理未保存修改' });
  await expect(discard).toBeVisible();
  await discard.getByRole('button', { name: '继续编辑', exact: true }).click();
  await expect(settings.getByLabel('模型名')).toHaveValue('unsaved-model');
  await settings.getByRole('button', { name: '关闭设置', exact: true }).click();
  await discard.getByRole('button', { name: '放弃修改并关闭', exact: true }).click();
  await expect(settings).toBeHidden();
  const calls = await page.evaluate(() => (window as unknown as { __SETTINGS_CALLS__: { command: string }[] }).__SETTINGS_CALLS__);
  expect(calls.filter(item => item.command === 'save_model_config')).toHaveLength(1);
  expect(calls.filter(item => item.command === 'test_model_config')).toHaveLength(1);
  expect(calls.some(item => ['send_chat_message', 'connect_server', 'connect_saved_profile', 'send_user_input'].includes(item.command))).toBe(false);
  expect(errors).toEqual([]);
});
