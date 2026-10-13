import { test, expect } from '@playwright/test';

test('browser preview: split layout, honest capabilities, isolated drafts and modal focus', async ({ page }, testInfo) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('/');
  await expect(page.getByText('网页预览 · 模型连接不可用')).toBeVisible();
  await expect(page.getByRole('button', { name: '确认并发送', exact: true })).toBeDisabled();
  const terminal = await page.getByRole('region', { name: 'SSH 终端' }).boundingBox();
  const chat = await page.getByRole('complementary', { name: 'AI 聊天工作区' }).boundingBox();
  expect(terminal).not.toBeNull(); expect(chat).not.toBeNull();
  expect(chat!.x).toBeGreaterThan(terminal!.x); expect(chat!.width).toBeGreaterThanOrEqual(319);
  await expect(page.getByRole('textbox', { name: '聊天消息' })).toBeInViewport();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: testInfo.outputPath('browser-preview-layout.png'), fullPage: true });

  await page.getByRole('textbox', { name: '聊天消息' }).fill('未发送草稿 A');
  await page.getByRole('button', { name: '新建聊天', exact: true }).click();
  await expect(page.getByRole('textbox', { name: '聊天消息' })).toHaveValue('');
  await page.getByRole('textbox', { name: '聊天消息' }).fill('未发送草稿 B');
  await page.getByRole('button', { name: '打开聊天列表', exact: true }).click();
  await page.getByRole('button', { name: '新任务 1', exact: true }).click();
  await expect(page.getByRole('textbox', { name: '聊天消息' })).toHaveValue('未发送草稿 A');
  await page.getByRole('textbox', { name: '聊天消息' }).press('Control+Enter');
  await expect(page.getByRole('article', { name: '模型回复' })).toHaveCount(0);

  await page.getByRole('button', { name: '打开设置', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: '工作空间设置' });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('button', { name: '重新加载模型配置' })).toBeDisabled();
  await expect(dialog.getByText(/请勿把 API Key、SSH 密码/)).toBeVisible();
  const close = dialog.getByRole('button', { name: '关闭对话框', exact: true });
  await expect(close).toBeFocused();
  await close.press('Shift+Tab');
  await expect(dialog.getByRole('button', { name: '关闭设置', exact: true })).toBeFocused();
  await page.keyboard.press('Tab'); await expect(close).toBeFocused();
  await page.screenshot({ path: testInfo.outputPath('browser-preview-settings.png'), fullPage: true });
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(page.getByRole('button', { name: '打开设置', exact: true })).toBeFocused();
  expect(errors).toEqual([]);
});

test('synthetic IPC mock: explicit recipient consent and escaped streamed text', async ({ page }, testInfo) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => {
    const callbacks = new Map<number, (event: unknown) => void>();
    const listeners = new Map<string, Set<number>>();
    const calls: string[] = [];
    let nextId = 1;
    const config = { path: '/synthetic-preview/model.toml', revision: '1', ready: true, destination: 'https://provider.example.invalid/v1/chat/completions', model: 'synthetic-preview-model', consented: false, error: null, warning: null, context_policy: 'explicit_chat_only', persistence: 'memory_only' };
    const chat = { chat_id: 'preview-chat', task_id: 'preview-chat', title: '新任务', revision: '1', active_run_id: null as string | null, messages: [] as Array<{ id: string; role: string; text: string; run_id: string; state: string; error: null; model?: string; destination?: string }> };
    const emit = (event: string, payload: unknown) => listeners.get(event)?.forEach(id => callbacks.get(id)?.({ event, id, payload: structuredClone(payload) }));
    const unlisten = (event: string, id: number) => { listeners.get(event)?.delete(id); callbacks.delete(id); };
    const root = window as unknown as Record<string, unknown>;
    root.isTauri = true;
    root.__SYNTHETIC_IPC_CALLS__ = calls;
    root.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: unlisten };
    root.__TAURI_INTERNALS__ = {
      transformCallback: (callback: (event: unknown) => void) => { const id = nextId++; callbacks.set(id, callback); return id; },
      unregisterCallback: (id: number) => callbacks.delete(id),
      invoke: async (cmd: string, args: Record<string, unknown>) => {
        calls.push(cmd);
        if (cmd === 'plugin:event|listen') { const name = String(args.event); if (!listeners.has(name)) listeners.set(name, new Set()); listeners.get(name)!.add(Number(args.handler)); return args.handler; }
        if (cmd === 'plugin:event|unlisten') { unlisten(String(args.event), Number(args.eventId)); return; }
        if (cmd === 'get_session_snapshot') return { session: null, state: 'disconnected', revision: '1' };
        if (cmd === 'get_model_config_status') return structuredClone(config);
        if (cmd === 'list_chats') return [structuredClone(chat)];
        if (cmd === 'get_chat') return structuredClone(chat);
        if (cmd === 'confirm_model_destination') {
          if (args.revision !== config.revision || args.destination !== config.destination) throw new Error('Stale preview consent');
          config.revision = '2'; config.consented = true; emit('model:event', { kind: 'config_updated', config }); return structuredClone(config);
        }
        if (cmd === 'send_chat_message') {
          if (!config.consented) throw new Error('Preview consent is required');
          chat.revision = '2'; chat.title = '解释一个只读检查步骤'; chat.active_run_id = 'preview-run';
          chat.messages = [
            { id: 'preview-user', role: 'user', text: String(args.text), run_id: 'preview-run', state: 'complete', error: null },
            { id: 'preview-assistant', role: 'assistant', text: '', run_id: 'preview-run', state: 'streaming', error: null, model: config.model, destination: config.destination },
          ];
          setTimeout(() => { chat.revision = '3'; chat.messages[1].text = '这是用于界面测试的合成流式回复。\n\n<script>window.unsafe = true</script>\n\n没有连接或操作任何服务器。'; chat.messages[1].state = 'complete'; chat.active_run_id = null; emit('model:event', { kind: 'chat_updated', chat }); }, 250);
          return structuredClone(chat);
        }
        throw new Error(`Unexpected synthetic IPC command: ${cmd}`);
      },
    };
  });
  await page.goto('/');
  await page.getByRole('textbox', { name: '聊天消息' }).fill('解释一个只读检查步骤。');
  await page.getByRole('button', { name: '确认并发送', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: '确认模型接收方' });
  await expect(dialog.getByText('https://provider.example.invalid/v1/chat/completions')).toBeVisible();
  await expect(dialog.getByRole('button', { name: '同意并发送' })).toBeDisabled();
  expect(await page.evaluate(() => (window as unknown as { __SYNTHETIC_IPC_CALLS__: string[] }).__SYNTHETIC_IPC_CALLS__.includes('send_chat_message'))).toBe(false);
  await page.screenshot({ path: testInfo.outputPath('synthetic-ipc-consent.png'), fullPage: true });
  await dialog.getByRole('checkbox').check(); await dialog.getByRole('button', { name: '同意并发送' }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByRole('article', { name: '模型回复' })).toContainText('这是用于界面测试的合成流式回复。');
  await expect(page.getByRole('article', { name: '模型回复' })).toContainText('<script>window.unsafe = true</script>');
  expect(await page.evaluate(() => (window as unknown as { unsafe?: boolean }).unsafe)).toBeUndefined();
  await expect(page.getByText('无生成中的聊天')).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('synthetic-ipc-completed-chat.png'), fullPage: true });
  expect(errors).toEqual([]);
});
