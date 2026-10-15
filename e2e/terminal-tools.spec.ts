import { test, expect } from '@playwright/test';

// Real xterm rendering with synthetic IPC, not live SSH or native clipboard evidence.
test('synthetic IPC: real terminal search, reviewed attachment and persistent display settings', async ({ page }, testInfo) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => {
    const callbacks = new Map<number, (event: unknown) => void>();
    const listeners = new Map<string, Set<number>>(); let nextId = 1;
    const source = { session_id: 'synthetic-terminal', generation: '1', host: 'synthetic.example', port: 22, username: 'tester' };
    const session = { session_id: source.session_id, generation: source.generation };
    const config = { path: '/synthetic/model.toml', file_revision: 'file-1', revision: '1', base_url: 'https://provider.example.invalid/v1', key_present: true, path_overridden: false, ready: true, destination: 'https://provider.example.invalid/v1', model: 'synthetic-model', consented: true, error: null, warning: null, context_policy: 'explicit_chat_only', persistence: 'memory_only' };
    const chat = { chat_id: 'synthetic-chat', task_id: 'synthetic-chat', title: '新任务', source, revision: '1', active_run_id: null, messages: [] as unknown[] };
    const calls: Array<{ cmd: string; args: Record<string, unknown> }> = [];
    const emit = (event: string, payload: unknown) => listeners.get(event)?.forEach(id => callbacks.get(id)?.({ event, id, payload: structuredClone(payload) }));
    const unlisten = (event: string, id: number) => { listeners.get(event)?.delete(id); callbacks.delete(id); };
    const root = window as unknown as Record<string, unknown>;
    root.isTauri = true;
    root.__TERMINAL_TOOLS_CALLS__ = calls;
    root.__TERMINAL_TOOLS_OUTPUT__ = () => emit('ssh:event', { type: 'terminal.output', session, seq: '1', data_base64: btoa('alpha output\r\nbeta output\r\nalpha next\r\n'), stream: 'stdout' });
    root.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: unlisten };
    root.__TAURI_INTERNALS__ = {
      transformCallback: (callback: (event: unknown) => void) => { const id = nextId++; callbacks.set(id, callback); return id; },
      unregisterCallback: (id: number) => callbacks.delete(id),
      invoke: async (cmd: string, args: Record<string, unknown>) => {
        calls.push({ cmd, args });
        if (cmd === 'plugin:event|listen') { const name = String(args.event); if (!listeners.has(name)) listeners.set(name, new Set()); listeners.get(name)!.add(Number(args.handler)); return args.handler; }
        if (cmd === 'plugin:event|unlisten') { unlisten(String(args.event), Number(args.eventId)); return; }
        if (cmd === 'get_session_snapshot') return { session, state: 'connected', host: source.host, port: source.port, username: source.username, revision: '1' };
        if (cmd === 'get_window_launch_target') return { profile_id: null, new_connection: false };
        if (cmd === 'get_history_status') return { enabled: false, manageable: false, available: true, plaintext: true, revision: '1', error: null };
        if (cmd === 'get_shell_integration_status') return { session, phase: 'unintegrated', sequence: 0, revision: 1, shadowed_commands: [] };
        if (cmd === 'resize_terminal' || cmd === 'ack_terminal_output') return;
        if (cmd === 'list_terminal_tasks') return [];
        if (cmd === 'get_model_config_status') return structuredClone(config);
        if (cmd === 'list_chats') return [structuredClone(chat)];
        if (cmd === 'get_chat') return structuredClone(chat);
        if (cmd === 'send_chat_message') {
          chat.revision = '2'; chat.messages = [{ id: 'sent-one', role: 'user', text: args.text, state: 'complete', run_id: 'run-one', error: null }];
          return structuredClone(chat);
        }
        throw new Error(`Unexpected synthetic terminal command: ${cmd}`);
      },
    };
  });
  await page.goto('/');
  await expect(page.getByLabel('聊天来源')).toContainText('synthetic-terminal');
  await page.evaluate(() => (window as unknown as { __TERMINAL_TOOLS_OUTPUT__: () => void }).__TERMINAL_TOOLS_OUTPUT__());
  await expect.poll(async () => page.evaluate(() => (window as unknown as { __TERMINAL_TOOLS_CALLS__: { cmd: string }[] }).__TERMINAL_TOOLS_CALLS__.some(call => call.cmd === 'ack_terminal_output'))).toBe(true);
  await page.locator('.xterm-helper-textarea').press('Control+Shift+F');
  const search = page.getByRole('search', { name: '搜索终端输出' });
  await search.getByRole('textbox', { name: '搜索终端文本' }).fill('alpha');
  await expect(search.getByRole('status')).toHaveText('1 / 2');
  await search.getByRole('button', { name: '下一个匹配' }).click();
  await expect(search.getByRole('status')).toHaveText('2 / 2');
  await search.getByRole('button', { name: '上一个匹配' }).click();
  await expect(search.getByRole('status')).toHaveText('1 / 2');
  await page.getByRole('button', { name: '选区交给 AI…' }).click();
  const attachment = page.getByRole('region', { name: '终端选区附件草稿' });
  await expect(attachment).toContainText('tester@synthetic.example:22');
  await expect(attachment.getByRole('textbox')).toHaveValue('alpha');
  await expect(page.getByRole('button', { name: '发送', exact: true })).toBeDisabled();
  await attachment.getByRole('textbox').fill('alpha (reviewed)');
  await expect(attachment).toContainText('已由你编辑');
  await attachment.getByRole('checkbox').check();
  await page.getByRole('textbox', { name: '聊天消息' }).fill('解释这个输出');
  await page.screenshot({ path: testInfo.outputPath('synthetic-terminal-selection-review.png'), fullPage: true });
  await page.getByRole('button', { name: '发送', exact: true }).click();
  await expect(attachment).toHaveCount(0);
  const calls = await page.evaluate(() => (window as unknown as { __TERMINAL_TOOLS_CALLS__: Array<{ cmd: string; args: Record<string, unknown> }> }).__TERMINAL_TOOLS_CALLS__);
  const sends = calls.filter(call => call.cmd === 'send_chat_message');
  expect(sends).toHaveLength(1); expect(sends[0].args.text).toContain('alpha (reviewed)'); expect(sends[0].args.text).toContain('"session_id":"synthetic-terminal"');
  expect(calls.filter(call => ['start_terminal_task', 'send_user_input'].includes(call.cmd))).toHaveLength(0);
  await search.getByRole('textbox').press('Escape'); await expect(search).toHaveCount(0);
  await page.getByRole('button', { name: '打开设置', exact: true }).click();
  const settings = page.getByRole('dialog', { name: '工作空间设置' });
  await settings.getByRole('button', { name: '终端', exact: true }).click();
  await settings.getByRole('spinbutton', { name: '终端字体大小' }).fill('18');
  await settings.getByRole('combobox', { name: '终端回滚行数' }).selectOption('1000');
  await settings.getByRole('button', { name: '关闭设置', exact: true }).click();
  await page.reload();
  await page.getByRole('button', { name: '打开设置', exact: true }).click();
  await settings.getByRole('button', { name: '终端', exact: true }).click();
  await expect(settings.getByRole('spinbutton', { name: '终端字体大小' })).toHaveValue('18');
  await expect(settings.getByRole('combobox', { name: '终端回滚行数' })).toHaveValue('1000');
  expect(errors).toEqual([]);
});
