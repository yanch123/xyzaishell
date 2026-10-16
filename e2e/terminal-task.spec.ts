import { test, expect } from '@playwright/test';

test('synthetic unified task: scope consent, painted command, approval and honest unknown completion', async ({ page }, testInfo) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => {
    const callbacks = new Map<number, (event: unknown) => void>();
    const listeners = new Map<string, Set<number>>();
    const calls: Array<{ cmd: string; args: Record<string, unknown> }> = [];
    let nextId = 1;
    const config = { path: '/synthetic-preview/model.toml', revision: '1', ready: true, destination: 'https://provider.example.invalid/v1/chat/completions', model: 'synthetic-preview-model', consented: false, error: null, warning: null, context_policy: 'explicit_chat_only', persistence: 'memory_only' };
    const session = { session_id: 'synthetic-ssh', generation: '1' };
    const chat = { chat_id: 'synthetic-chat', task_id: 'synthetic-chat', title: '新任务', revision: '1', active_run_id: null, messages: [] };
    type Action = { action_id: string; display_revision: string; command_text: string; append_enter: boolean; digest: string; policy: string; reason: string; approval_id: string | null; expires_at_unix_ms: number; displayed: boolean; delivery: string };
    type Task = { task_context?: Record<string, unknown>; chat_id: string; task_id: string; run_id: string; session: typeof session; target: { host: string; username: string; port: number }; revision: string; phase: string; instruction: string; assistant_text: string; action: Action; actions: Action[]; error: null; owner: boolean; remote_outcome: string; terminal_ready: boolean; round: number };
    let task: Task | null = null;
    const emit = (event: string, payload: unknown) => listeners.get(event)?.forEach(id => callbacks.get(id)?.({ event, id, payload: structuredClone(payload) }));
    const unlisten = (event: string, id: number) => { listeners.get(event)?.delete(id); callbacks.delete(id); };
    const update = () => { if (task) { task.revision = String(Number(task.revision) + 1); emit('agent:event', { kind: 'task_updated', task }); } return structuredClone(task); };
    const root = window as unknown as Record<string, unknown>;
    root.isTauri = true; root.__SYNTHETIC_TASK_CALLS__ = calls;
    root.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: unlisten };
    root.__TAURI_INTERNALS__ = {
      transformCallback: (callback: (event: unknown) => void) => { const id = nextId++; callbacks.set(id, callback); return id; },
      unregisterCallback: (id: number) => callbacks.delete(id),
      invoke: async (cmd: string, args: Record<string, unknown>) => {
        calls.push({ cmd, args });
        if (cmd === 'plugin:event|listen') { const name = String(args.event); if (!listeners.has(name)) listeners.set(name, new Set()); listeners.get(name)!.add(Number(args.handler)); return args.handler; }
        if (cmd === 'plugin:event|unlisten') { unlisten(String(args.event), Number(args.eventId)); return; }
        if (cmd === 'get_session_snapshot') return { session, state: 'connected', revision: '1', host: 'synthetic-server.invalid', username: 'synthetic-user', port: 22 };
        if (cmd === 'resize_terminal' || cmd === 'ack_terminal_output' || cmd === 'send_user_input') return;
        if (cmd === 'get_window_launch_target') return { profile_id: null, new_connection: false };
        if (cmd === 'get_history_status') return { enabled: false, manageable: false, available: true, plaintext: true, revision: '1', error: null };
        if (cmd === 'get_shell_integration_status') return { session, phase: 'ready', shell_id: 'synthetic-shell', revision: 1, sequence: 0, shadowed_commands: [] };
        if (cmd === 'create_chat') return structuredClone(chat);
        if (cmd === 'get_model_config_status') return structuredClone(config);
        if (cmd === 'list_chats') return [structuredClone(chat)];
        if (cmd === 'get_chat') return structuredClone(chat);
        if (cmd === 'list_terminal_tasks') return task ? [structuredClone(task)] : [];
        if (cmd === 'get_terminal_task') return structuredClone(task);
        const request = args.request as Record<string, unknown> | undefined;
        if (cmd === 'confirm_conversation_scope') {
          if (JSON.stringify(request?.session) !== JSON.stringify(session) || request?.revision !== config.revision || request?.destination !== config.destination) throw new Error('Missing target scope');
          config.consented = true; return structuredClone(config);
        }
        if (cmd === 'submit_conversation_turn') {
          if (!config.consented || args.revision !== config.revision || JSON.stringify(args.session) !== JSON.stringify(session)) throw new Error('Missing scope consent');
          task = { chat_id: chat.chat_id, task_id: chat.task_id, run_id: 'synthetic-run', session, target: { host: 'synthetic-server.invalid', username: 'synthetic-user', port: 22 }, revision: '1', phase: 'awaiting_display', instruction: String(args.text), assistant_text: '<script>window.unsafe = true</script> 合成模型分析，仅验证界面。', action: { action_id: 'synthetic-action-1', display_revision: '1', command_text: 'uname -a', append_enter: true, digest: 'synthetic-binding-1', policy: 'auto_read_only', reason: '合成测试：基础策略允许。', approval_id: null, expires_at_unix_ms: Date.now() + 120000, displayed: false, delivery: 'not_sent' }, actions: [], error: null, owner: true, remote_outcome: 'not_sent', terminal_ready: true, round: 1 };
          task.task_context = { task_id: task.task_id, run_id: task.run_id, session, summary: '合成任务摘要，仅测试界面', confirmed_facts: [], findings: [], hypotheses: [{ id: 'guess-1', text: '尚未核实系统版本', evidence_command_ids: [] }], remaining_steps: [{ id: 'step-1', text: '核查系统版本', status: 'pending', evidence_command_ids: [] }], evidence: [], truncated: false };
          return { kind: 'terminal', task: update() };
        }
        if (cmd === 'ack_terminal_action' && task) {
          const card = document.querySelector(`[data-action-id="${task.action.action_id}"]`);
          if (request?.action_id !== task.action.action_id || document.querySelector('[role="dialog"]') || card?.querySelector('.exact-command')?.textContent !== task.action.command_text || !card?.getClientRects().length) throw new Error('Command must be displayed before dispatch');
          task.action.displayed = true;
          if (task.action.policy === 'auto_read_only') { task.action.delivery = 'write_accepted'; task.remote_outcome = 'unknown'; task.terminal_ready = false; task.phase = 'needs_user'; }
          else task.phase = 'waiting_approval';
          return update();
        }
        if (cmd === 'continue_terminal_task' && task) {
          // Compatibility field records this explicit recovery click; it is not ShellReady evidence.
          if (request?.readiness_confirmed !== true) throw new Error('Missing explicit recovery request');
          task.actions.push(structuredClone(task.action)); task.round = 2; task.terminal_ready = true;
          task.phase = 'awaiting_display'; task.assistant_text = '第二轮合成分析：需要单次授权。';
          task.action = { ...task.action, action_id: 'synthetic-action-2', command_text: 'touch ./synthetic-marker', digest: 'synthetic-binding-2', policy: 'require_approval', reason: '将修改文件系统，需单次确认。', approval_id: 'synthetic-approval-2', displayed: false, delivery: 'not_sent' };
          return update();
        }
        if (cmd === 'decide_terminal_action' && task) {
          if (request?.action_id !== task.action.action_id || request?.approval_id !== task.action.approval_id || !task.action.displayed) throw new Error('Stale synthetic approval');
          if (request.decision !== 'deny') throw new Error('This fixture expects rejection');
          task.phase = 'cancelled'; task.owner = false;
          return update();
        }
        if (cmd === 'cancel_terminal_task' && task) { task.phase = 'cancelled'; task.owner = false; return update(); }
        throw new Error(`Unexpected synthetic IPC: ${cmd}`);
      },
    };
  });
  await page.goto('/');
  await expect(page.getByRole('button', { name: '可执行 · Shell 已就绪', exact: true })).toBeVisible();
  await expect(page.getByRole('tab', { name: '执行终端任务' })).toHaveCount(0);
  await page.getByRole('textbox', { name: '聊天消息' }).fill('执行合成检查任务，不连接真实服务器。');
  await page.getByRole('button', { name: '确认并发送', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: '确认模型接收方' });
  await expect(dialog.getByText('https://provider.example.invalid/v1/chat/completions')).toBeVisible();
  const launch = dialog.getByRole('button', { name: '同意并发送' });
  await expect(launch).toBeDisabled();
  await expect(dialog.getByRole('checkbox')).toHaveCount(1);
  await dialog.getByRole('checkbox').check();
  await expect(launch).toBeEnabled();
  await page.screenshot({ path: testInfo.outputPath('synthetic-task-consent.png'), fullPage: true });
  await launch.click(); await expect(dialog).toBeHidden();
  const card = page.getByRole('group', { name: '精确终端命令' });
  // The compact window must leave enough actual task space to show both the
  // exact target and command. Do not bypass the production display-ACK gate.
  await expect.poll(async () => (await page.locator('.chat-scroll').boundingBox())?.height ?? 0).toBeGreaterThanOrEqual(220);
  await card.locator('.action-target').scrollIntoViewIfNeeded();
  await expect(card.locator('.action-target')).toBeInViewport({ ratio: 1 });
  await expect(card.getByLabel('准确命令文本')).toBeInViewport({ ratio: 1 });
  await expect(card.getByText('SSH 接受写入 · 不代表命令成功')).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('synthetic-task-dispatched.png'), fullPage: true });
  await expect(page.getByRole('article', { name: '任务模型分析' })).toContainText('<script>window.unsafe = true</script>');
  expect(await page.evaluate(() => (window as unknown as { unsafe?: boolean }).unsafe)).toBeUndefined();
  const proceed = page.getByRole('button', { name: '核对状态并继续' });
  await expect(proceed).toBeEnabled();
  await expect(page.getByRole('checkbox', { name: '我已检查终端，并确认当前可接收下一步命令。' })).toHaveCount(0);
  await proceed.click();
  await card.locator('.action-target').scrollIntoViewIfNeeded();
  await expect(card.locator('.action-target')).toBeInViewport({ ratio: 1 });
  await expect(card.getByLabel('准确命令文本')).toBeInViewport({ ratio: 1 });
  await expect(page.getByRole('button', { name: '仅批准这条命令' })).toBeEnabled();
  await expect(card.getByLabel('准确命令文本')).toHaveText('touch ./synthetic-marker');
  await page.screenshot({ path: testInfo.outputPath('synthetic-task-single-approval.png'), fullPage: true });
  const beforeDecision = await page.evaluate(() => (window as unknown as { __SYNTHETIC_TASK_CALLS__: Array<{ cmd: string }> }).__SYNTHETIC_TASK_CALLS__);
  expect(beforeDecision.filter(call => call.cmd === 'decide_terminal_action')).toHaveLength(0);
  await page.getByRole('button', { name: '拒绝本次命令' }).click();
  await expect(page.getByText('AI 已停止', { exact: true })).toBeVisible();
  const beforeInterrupt = await page.evaluate(() => (window as unknown as { __SYNTHETIC_TASK_CALLS__: Array<{ cmd: string }> }).__SYNTHETIC_TASK_CALLS__.map(call => call.cmd));
  expect(beforeInterrupt.filter(cmd => cmd === 'submit_conversation_turn')).toHaveLength(1);
  expect(beforeInterrupt.filter(cmd => cmd === 'ack_terminal_action')).toHaveLength(2);
  expect(beforeInterrupt.filter(cmd => cmd === 'decide_terminal_action')).toHaveLength(1);
  expect(beforeInterrupt).not.toContain('send_user_input'); expect(beforeInterrupt).not.toContain('send_chat_message');
  await page.getByRole('button', { name: '发送 Ctrl+C' }).click();
  await expect.poll(async () => page.evaluate(() => (window as unknown as { __SYNTHETIC_TASK_CALLS__: Array<{ cmd: string }> }).__SYNTHETIC_TASK_CALLS__.filter(call => call.cmd === 'send_user_input').length)).toBe(1);
  await page.locator('.task-context-panel > summary').click();
  await expect(page.getByText('核查系统版本', { exact: true })).toBeVisible();
  await expect(page.getByText('尚未核实系统版本', { exact: true })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('synthetic-task-context.png'), fullPage: true });
  expect(errors).toEqual([]);
});
