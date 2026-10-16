import { test, expect } from '@playwright/test';
type ChatSnapshot = {
  chat_id: string; task_id: string; title: string; revision: string; active_run_id: string | null;
  messages: Array<{ id: string; role: 'user' | 'assistant'; text: string; run_id: string; state: 'complete'; error: null; destination?: null; model?: null }>;
};
type TerminalTaskSnapshot = {
  chat_id: string; task_id: string; run_id: string; session: { session_id: string; generation: string };
  target: { host: string; username: string; port: number }; revision: string; phase: string;
  instruction: string; assistant_text: string; owner: boolean; remote_outcome: string; terminal_ready: boolean; round: number; error: null; actions: unknown[];
  action: null | { action_id: string; display_revision: string; command_text: string; append_enter: true;
    digest: string; policy: 'auto_normal'; reason: string; approval_id: null; expires_at_unix_ms: number;
    displayed: boolean; delivery: string; command_completed?: boolean; exit_code?: number };
};

// Frontend regression only: native IPC, SSH evidence and model replies are synthetic.

test('synthetic unified conversation: greeting, displayed ordinary command and contextual follow-up share one composer', async ({ page }, testInfo) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => {
    const callbacks = new Map<number, (event: unknown) => void>();
    const listeners = new Map<string, Set<number>>();
    const calls: Array<{ cmd: string; args: Record<string, unknown> }> = [];
    let nextId = 1;
    const config = { path: '/synthetic-preview/model.toml', revision: '1', ready: true, destination: 'https://provider.example.invalid/v1/chat/completions', model: 'synthetic-preview-model', consented: false, error: null, warning: null, context_policy: 'explicit_chat_only', persistence: 'memory_only' };
    const session = { session_id: 'synthetic-ssh', generation: '1' };
    const chat: ChatSnapshot = { chat_id: 'synthetic-chat', task_id: 'synthetic-chat', title: '新任务', revision: '1', active_run_id: null, messages: [] };
    let task: TerminalTaskSnapshot | null = null;
    let turn = 0;
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
        if (cmd === 'resize_terminal' || cmd === 'ack_terminal_output') return;
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
          if (!config.consented || args.revision !== config.revision || args.destination !== config.destination || JSON.stringify(args.session) !== JSON.stringify(session)) throw new Error('No current target scope');
          if (turn === 2 && !chat.messages.some(message => message.text === 'Linux verified by uname.')) throw new Error('Prior tool result missing from conversation');
          turn++;
          const run = 'run-' + turn;
          chat.messages.push({ id: 'user-' + turn, role: 'user', text: String(args.text), run_id: run, state: 'complete', error: null, destination: null, model: null });
          task = { chat_id: chat.chat_id, task_id: chat.task_id, run_id: run, session, target: {host:'synthetic-server.invalid', username:'synthetic-user',port:22}, revision:String(turn*10), phase: turn===2?'awaiting_display':'completed', instruction:String(args.text), assistant_text: turn===1?'Hello, ready to help.':turn===2?'Inspecting server now.':'The server uses Linux.', action:turn===2?{action_id:'ordinary-2',display_revision:'1',command_text:'command uname -a',append_enter:true,digest:'digest',policy:'auto_normal',reason:'Inspection',approval_id:null,expires_at_unix_ms:Date.now()+120000,displayed:false,delivery:'not_sent'}:null, actions:[],error:null,owner:turn===2,remote_outcome:'not_sent',terminal_ready:true,round:1};
          if(turn!==2) chat.messages.push({id:'assistant-'+turn,role:'assistant',text:task.assistant_text,run_id:run,state:'complete',error:null,destination:null,model:null});
          chat.revision=String(turn*10); emit('model:event',{kind:'chat_updated',chat});
          return {kind:'terminal',task:update()};
        }
        if (cmd === 'ack_terminal_action' && task?.action) {
          const action = task.action;
          if (request?.task_id !== task.task_id || request?.run_id !== task.run_id || request?.action_id !== action.action_id || request?.display_revision !== action.display_revision || action.displayed) throw new Error('Stale or repeated display acknowledgement');
          const card = document.querySelector(`[data-action-id="${action.action_id}"]`);
          const viewport = card?.closest('.chat-scroll')?.getBoundingClientRect();
          const visible = (selector: string) => {
            const bounds = card?.querySelector(selector)?.getBoundingClientRect();
            return !!bounds && !!viewport && bounds.width > 0 && bounds.height > 0
              && bounds.bottom > Math.max(0, viewport.top) && bounds.top < Math.min(innerHeight, viewport.bottom);
          };
          if (document.visibilityState !== 'visible' || document.querySelector('[aria-modal="true"]') || card?.closest('[inert]')
              || card?.querySelector('.exact-command')?.textContent !== action.command_text
              || !card?.querySelector('.action-target')?.textContent?.includes('synthetic-user@synthetic-server.invalid:22')
              || !visible('.action-target') || !visible('.exact-command')) throw new Error('Exact command and target must be on screen before dispatch');
          // Simulate the backend result only after the production component's display ACK.
          action.displayed = true; action.delivery = 'write_accepted'; action.command_completed = true; action.exit_code = 0;
          task.owner = false; task.phase = 'completed'; task.remote_outcome = 'foreground_returned';
          task.assistant_text = 'Linux verified by uname.';
          chat.messages.push({ id: 'assistant-2', role: 'assistant', text: task.assistant_text, run_id: task.run_id, state: 'complete', error: null });
          chat.revision = '21'; emit('model:event', { kind: 'chat_updated', chat });
          return update();
        }
        throw new Error(`Unexpected synthetic IPC: ${cmd}`);
      },
    };
  });
  await page.goto('/');
  const composer = page.getByRole('textbox', { name: '聊天消息' });
  const scope = page.getByRole('dialog', { name: '确认模型接收方' });
  const recordedCalls = () => page.evaluate(() => (window as unknown as {
    __SYNTHETIC_TASK_CALLS__: Array<{ cmd: string; args: Record<string, unknown> }>;
  }).__SYNTHETIC_TASK_CALLS__);
  await expect(composer).toHaveCount(1);
  await expect(page.getByRole('tab', { name: '执行终端任务' })).toHaveCount(0);
  await composer.fill('Hello');
  await page.getByRole('button', { name: '确认并发送', exact: true }).click();
  await expect(scope.getByRole('button', { name: '同意并发送' })).toBeDisabled();
  expect((await recordedCalls()).filter(call => call.cmd === 'submit_conversation_turn')).toHaveLength(0);
  await scope.getByRole('checkbox').check();
  await scope.getByRole('button', { name: '同意并发送' }).click();
  await expect(scope).toBeHidden();
  await expect(page.getByText('Hello, ready to help.', { exact: true })).toHaveCount(1);
  await expect(page.getByText('Hello', { exact: true })).toHaveCount(1);
  expect((await recordedCalls()).filter(call => call.cmd === 'ack_terminal_action')).toHaveLength(0);

  await composer.fill('Inspect the server');
  await page.getByRole('button', { name: '发送', exact: true }).click();
  const card = page.getByRole('group', { name: '精确终端命令' });
  await card.locator('.action-target').scrollIntoViewIfNeeded();
  await expect(card.getByLabel('准确命令文本')).toHaveText('command uname -a');
  await expect(page.getByText('Linux verified by uname.', { exact: true })).toHaveCount(1);
  await expect(page.getByText('Inspect the server', { exact: true })).toHaveCount(1);
  await expect(page.getByRole('button', { name: '仅批准这条命令' })).toHaveCount(0);
  await expect(scope).toBeHidden();

  await composer.fill('What OS did you find?');
  await page.getByRole('button', { name: '发送', exact: true }).click();
  await expect(page.getByText('The server uses Linux.', { exact: true })).toHaveCount(1);
  await expect(scope).toBeHidden();
  for (const text of ['Hello', 'Hello, ready to help.', 'Inspect the server', 'Linux verified by uname.', 'What OS did you find?', 'The server uses Linux.']) {
    await expect(page.getByText(text, { exact: true })).toHaveCount(1);
  }
  await expect(page.getByRole('textbox')).toHaveCount(1);
  await expect(page.getByRole('article', { name: '你的消息' })).toHaveCount(3);
  await expect(page.getByRole('article', { name: '模型回复' })).toHaveCount(3);
  const calls = await recordedCalls();
  expect(calls.filter(call => call.cmd === 'confirm_conversation_scope')).toHaveLength(1);
  expect(calls.filter(call => call.cmd === 'submit_conversation_turn').map(call => call.args.text)).toEqual(['Hello', 'Inspect the server', 'What OS did you find?']);
  expect(calls.filter(call => call.cmd === 'ack_terminal_action')).toHaveLength(1);
  expect(calls.filter(call => ['start_terminal_task', 'send_chat_message', 'send_user_input', 'continue_terminal_task', 'decide_terminal_action'].includes(call.cmd))).toHaveLength(0);
  await page.screenshot({ path: testInfo.outputPath('synthetic-unified-three-turn-conversation.png'), fullPage: true });
  expect(errors).toEqual([]);
});
