import { test, expect } from '@playwright/test';

test('browser preview: split layout, honest capabilities, isolated drafts and nonmodal settings', async ({ page }, testInfo) => {
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
  await page.getByRole('button', { name: /^新任务 1(?: 有未发送草稿)?$/ }).click();
  await expect(page.getByRole('textbox', { name: '聊天消息' })).toHaveValue('未发送草稿 A');
  await page.getByRole('textbox', { name: '聊天消息' }).press('Control+Enter');
  await expect(page.getByRole('article', { name: '模型回复' })).toHaveCount(0);

  await page.getByRole('button', { name: '打开设置', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: '工作空间设置' });
  await expect(dialog).toBeVisible();
  await expect(dialog).toHaveAttribute('aria-modal', 'false');
  await expect(dialog.getByLabel('Base URL', { exact: false })).toBeDisabled();
  await expect(dialog.getByRole('button', { name: '保存', exact: true })).toBeDisabled();
  expect(await page.locator('.app-shell').evaluate(element => element.hasAttribute('inert'))).toBe(false);
  await expect(page.locator('.modal-backdrop')).toHaveCount(0);
  await page.getByRole('button', { name: '打开设置', exact: true }).click();
  await expect(page.getByRole('dialog', { name: '工作空间设置' })).toHaveCount(1);
  await page.screenshot({ path: testInfo.outputPath('browser-preview-settings.png'), fullPage: true });
  await dialog.getByRole('button', { name: '关闭设置', exact: true }).click();
  await expect(dialog).toBeHidden();
  await page.getByRole('button', { name: '打开设置', exact: true }).click();
  await dialog.getByRole('button', { name: '通用', exact: true }).click();
  await dialog.getByRole('button', { name: '通用', exact: true }).press('Escape');
  await expect(dialog).toBeHidden();
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
    let turns = 0;
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
        if (cmd === 'get_window_launch_target') return { profile_id: null, new_connection: false };
        if (cmd === 'get_history_status') return { enabled: false, manageable: false, available: true, plaintext: true, revision: '1', error: null };
        if (cmd === 'list_terminal_tasks') return [];
        if (cmd === 'get_model_config_status') return structuredClone(config);
        if (cmd === 'list_chats') return [structuredClone(chat)];
        if (cmd === 'get_chat') return structuredClone(chat);
        if (cmd === 'confirm_conversation_scope') {
          const request = args.request as Record<string, unknown>;
          if (request.session !== null || request.revision !== config.revision || request.destination !== config.destination) throw new Error('Stale preview consent');
          config.revision = '2'; config.consented = true; emit('model:event', { kind: 'config_updated', config }); return structuredClone(config);
        }
        if (cmd === 'submit_conversation_turn') {
          if (typeof args.submissionId !== 'string' || !/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(args.submissionId)) throw new Error('Missing submission UUID');
          if (!config.consented) throw new Error('Preview consent is required');
          if (args.session !== null) throw new Error('Greeting must work without SSH');
          turns += 1;
          if (turns === 2 && chat.messages[1]?.state !== 'complete') throw new Error('Previous turn missing');
          chat.revision = String(Number(chat.revision) + 1); chat.title = '你好'; chat.active_run_id = `preview-run-${turns}`;
          chat.messages.push(
            { id: `preview-user-${turns}`, role: 'user', text: String(args.text), run_id: chat.active_run_id, state: 'complete', error: null },
            { id: `preview-assistant-${turns}`, role: 'assistant', text: '', run_id: chat.active_run_id, state: 'streaming', error: null, model: config.model, destination: config.destination },
          );
          setTimeout(() => { chat.revision = String(Number(chat.revision) + 1); chat.messages[chat.messages.length - 1].text = '这是用于界面测试的合成流式回复。\n\n<script>window.unsafe = true</script>\n\n没有连接或操作任何服务器。'; chat.messages[chat.messages.length - 1].state = 'complete'; chat.active_run_id = null; emit('model:event', { kind: 'chat_updated', chat }); }, 250);
          return { kind: 'chat', chat: structuredClone(chat) };
        }
        throw new Error(`Unexpected synthetic IPC command: ${cmd}`);
      },
    };
  });
  await page.goto('/');
  await expect(page.getByRole('tab', { name: '执行终端任务' })).toHaveCount(0);
  await expect(page.getByRole('textbox', { name: '终端任务指令' })).toHaveCount(0);
  await page.getByRole('textbox', { name: '聊天消息' }).fill('你好');
  await page.getByRole('button', { name: '确认并发送', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: '确认模型接收方' });
  await expect(dialog.getByText('https://provider.example.invalid/v1/chat/completions')).toBeVisible();
  await expect(dialog.getByRole('button', { name: '同意并发送' })).toBeDisabled();
  expect(await page.evaluate(() => (window as unknown as { __SYNTHETIC_IPC_CALLS__: string[] }).__SYNTHETIC_IPC_CALLS__.includes('submit_conversation_turn'))).toBe(false);
  await page.screenshot({ path: testInfo.outputPath('synthetic-ipc-consent.png'), fullPage: true });
  await dialog.getByRole('checkbox').check(); await dialog.getByRole('button', { name: '同意并发送' }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByRole('article', { name: '模型回复' })).toContainText('这是用于界面测试的合成流式回复。');
  await expect(page.getByRole('article', { name: '模型回复' })).toContainText('<script>window.unsafe = true</script>');
  expect(await page.evaluate(() => (window as unknown as { unsafe?: boolean }).unsafe)).toBeUndefined();
  await expect(page.getByRole('button', { name: '发送', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: '停止生成', exact: true })).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath('synthetic-ipc-completed-chat.png'), fullPage: true });
  await page.getByRole('textbox', { name: '聊天消息' }).fill('继续刚才的对话');
  await page.getByRole('button', { name: '发送', exact: true }).click();
  await expect(page.getByRole('article', { name: '模型回复' })).toHaveCount(2);
  await expect(page.getByRole('button', { name: '发送', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: '停止生成', exact: true })).toHaveCount(0);
  await expect(dialog).toBeHidden();
  const calls = await page.evaluate(() => (window as unknown as { __SYNTHETIC_IPC_CALLS__: string[] }).__SYNTHETIC_IPC_CALLS__);
  expect(calls.filter(cmd => cmd === 'confirm_conversation_scope')).toHaveLength(1);
  expect(calls.filter(cmd => cmd === 'submit_conversation_turn')).toHaveLength(2);
  expect(calls).not.toContain('start_terminal_task');
  expect(errors).toEqual([]);
});
