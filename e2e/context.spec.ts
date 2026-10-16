import { test, expect, type Page } from '@playwright/test';

// Synthetic IPC contract coverage only. Native estimator/compactor/archive
// correctness is covered by Rust tests and the installed-application workflow.
async function fixture(page: Page, history = false) {
  await page.addInitScript(({ history }) => {
    const callbacks = new Map<number, (event: unknown) => void>();
    const listeners = new Map<string, Set<number>>();
    let serial = 1;
    const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
    const config = { path: '/fixture/model.toml', revision: '1', file_revision: 'file-1', base_url: 'https://provider.example.invalid/v1', key_present: true, path_overridden: false, ready: true, destination: 'https://provider.example.invalid/v1/chat/completions', model: 'fixture-model', consented: true, error: null, warning: null, context_policy: 'explicit_chat_only', persistence: 'memory_only', context_auto_compact: true, context_auto_mode: 'llm', context_window_tokens: 256000, capacity_source: 'product_default' };
    const chat = { chat_id: 'context-chat', task_id: 'context-task', title: '上下文测试', revision: '1', active_run_id: null, messages: [{ id: 'user-1', role: 'user', text: '已提交消息', run_id: 'run-1', state: 'complete', error: null }, { id: 'assistant-1', role: 'assistant', text: '已完成回复', run_id: 'run-1', state: 'complete', error: null }] };
    const usage = { chat_id: chat.chat_id, context_revision: '1', config_revision: '1', projection_id: 'projection-1', request_kind: 'conversation', turn_id: 'turn-1', turn_phase: 'end_of_turn', measurement_scope: 'committed_context', prepared_at: '2026-10-16T18:00:00Z', status: 'ready', estimate: { method: 'heuristic', estimator_version: 'fixture-heuristic', allocation_method: 'normalized_estimate', estimated_input_tokens: 20000, conservative_input_tokens: 22000, categories: [{ category: 'rules_tools', estimated_tokens: 5000 }, { category: 'dialogue', estimated_tokens: 15000 }] }, budget: { context_window_tokens: 256000, capacity_source: 'product_default', output_reserve_tokens: 16000, safety_margin_tokens: 12800, input_budget_tokens: 227200, decision_input_tokens: 22000, input_occupancy_ratio: 22000 / 227200, after_turn_trigger_ratio: 0.8, in_turn_trigger_ratio: 0.95, effective_trigger_ratio: 0.8 }, byte_guards: [], last_request_usage: null, last_compaction_usage: null };
    let job: null | { job_id: string; chat_id: string; revision: string; state: string; mode: string; reason: string } = null;
    const root = window as unknown as Record<string, unknown>;
    root.__CONTEXT_CALLS__ = calls;
    root.__CONTEXT_UPDATE__ = (active: boolean) => {
      usage.context_revision = '2'; usage.turn_phase = active ? 'active_turn' : 'end_of_turn'; usage.budget.effective_trigger_ratio = active ? 0.95 : 0.8;
      listeners.get('model:event')?.forEach(id => callbacks.get(id)?.({ event: 'model:event', id, payload: { kind: 'context_usage_updated', usage: structuredClone(usage) } }));
    };
    root.isTauri = true;
    root.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: (event: string, id: number) => listeners.get(event)?.delete(id) };
    root.__TAURI_INTERNALS__ = {
      transformCallback: (callback: (event: unknown) => void) => { const id = serial++; callbacks.set(id, callback); return id; },
      unregisterCallback: (id: number) => callbacks.delete(id),
      invoke: async (command: string, args: Record<string, unknown> = {}) => {
        calls.push({ command, args: structuredClone(args) });
        if (command === 'plugin:event|listen') { const name = String(args.event); if (!listeners.has(name)) listeners.set(name, new Set()); listeners.get(name)!.add(Number(args.handler)); return args.handler; }
        if (command === 'plugin:event|unlisten') { listeners.get(String(args.event))?.delete(Number(args.eventId)); return; }
        if (command === 'get_session_snapshot') return { session: null, state: 'disconnected', revision: '1' };
        if (command === 'get_window_launch_target') return { profile_id: null, new_connection: false };
        if (command === 'get_history_status') return { enabled: history, manageable: history, available: true, plaintext: true, revision: '1', error: null };
        if (command === 'list_terminal_tasks') return [];
        if (command === 'list_saved_chats') return [{ chat_id: chat.chat_id, task_id: chat.task_id, title: chat.title, message_count: 130, revision: 'archive-1', updated_ms: 1, listed: true }];
        if (command === 'read_saved_chat_page') {
          if (args.expectedRevision !== 'archive-1' || args.limit !== 64) throw new Error('Wrong archive fence or limit');
          const end = args.beforeSequence == null ? 130 : Number(args.beforeSequence) - 1;
          const start = Math.max(1, end - 63);
          return { chat: { ...structuredClone(chat), restored_read_only: true, messages: Array.from({ length: end - start + 1 }, (_, index) => ({ id: `saved-${start + index}`, role: 'user', text: `归档消息 ${start + index}`, run_id: 'archived-run', state: 'complete', error: null })) }, total_messages: 130, next_before: start > 1 ? start : null };
        }
        if (command === 'get_model_config_status') return structuredClone(config);
        if (command === 'list_chats') return [structuredClone(chat)];
        if (command === 'get_chat') return structuredClone(chat);
        if (command === 'get_context_usage') return { ...structuredClone(usage), measurement_scope: args.draft ? 'draft_preview' : 'committed_context' };
        if (command === 'get_context_compaction') return structuredClone(job);
        if (command === 'compact_context') { job = { job_id: 'compact-1', chat_id: chat.chat_id, revision: '1', state: 'compressing', mode: String(args.mode), reason: 'manual' }; return structuredClone(job); }
        if (command === 'cancel_context_compaction') { if (!job || args.jobId !== job.job_id) throw new Error('Wrong compaction job'); job = { ...job, revision: '2', state: 'cancelled' }; return structuredClone(job); }
        throw new Error(`Unexpected context fixture IPC: ${command}`);
      },
    };
  }, { history });
  await page.goto('/');
  await expect(page.getByRole('button', { name: /^可用输入预算占用 ≈10%/ })).toBeVisible();
  await page.getByRole('button', { name: /^可用输入预算占用/ }).click();
}

test('synthetic context: default capacity, honest source, categories and phase thresholds', async ({ page }, testInfo) => {
  await fixture(page);
  const panel = page.getByRole('dialog', { name: '上下文占用与压缩' });
  await expect(panel).toHaveAttribute('aria-modal', 'false');
  await expect(panel).toContainText('保守估算 · 并非供应商实报');
  await expect(panel).toContainText('回复后 80%，任务中 95% · 当前回复后 80%');
  await expect(panel.getByRole('region', { name: '输入类别构成（估算）' })).toContainText('占输入 25%');
  await panel.getByText('详情', { exact: true }).click();
  await expect(panel).toContainText('256K = 256000 tokens，是产品默认配置');
  await expect(panel).toContainText('未知 · 未收到供应商用量');
  await expect(panel).not.toContainText(/(?:目标|压缩到|压到|降至|降到)\s*50%|50%\s*(?:目标|固定)/);
  await page.evaluate(() => (window as unknown as { __CONTEXT_UPDATE__: (active: boolean) => void }).__CONTEXT_UPDATE__(true));
  await expect(panel).toContainText('当前任务中 95%');
  const rect = await panel.boundingBox();
  expect(rect).not.toBeNull();
  expect(rect!.x).toBeGreaterThanOrEqual(0);
  expect(rect!.x + rect!.width).toBeLessThanOrEqual(page.viewportSize()!.width);
  await page.screenshot({ path: testInfo.outputPath('synthetic-context-popover.png'), fullPage: true });
  await panel.press('Escape');
  await expect(panel).toBeHidden();
});

for (const mode of ['llm', 'quick']) test(`synthetic context: ${mode} compaction progress/cancel excludes unsent draft`, async ({ page }) => {
  await fixture(page);
  const panel = page.getByRole('dialog', { name: '上下文占用与压缩' });
  await panel.getByRole('button', { name: '关闭上下文' }).click();
  await page.getByRole('textbox', { name: '聊天消息' }).fill('草稿不能发送到压缩请求');
  await page.getByRole('button', { name: /^可用输入预算占用/ }).click();
  await expect(panel).toContainText('含未发送草稿 · 仅在本机预估');
  await panel.getByLabel('本次压缩方式').selectOption(mode);
  await panel.getByRole('button', { name: '立即压缩', exact: true }).click();
  await expect(page.getByRole('button', { name: /^可用输入预算占用/ })).toContainText('正在压缩…');
  await panel.getByRole('button', { name: '取消压缩', exact: true }).click();
  await expect(panel).toContainText('压缩已取消，原上下文保留');
  await expect(page.getByRole('textbox', { name: '聊天消息' })).toHaveValue('草稿不能发送到压缩请求');
  const calls = await page.evaluate(() => (window as unknown as { __CONTEXT_CALLS__: Array<{ command: string; args: Record<string, unknown> }> }).__CONTEXT_CALLS__);
  const compactions = calls.filter(call => call.command === 'compact_context');
  expect(compactions).toHaveLength(1);
  expect(compactions[0].args).toEqual({ chatId: 'context-chat', expectedContextRevision: '1', mode, clientRequestId: expect.stringMatching(/^[0-9a-f-]{36}$/) });
  expect(calls.some(call => call.command === 'submit_conversation_turn' || call.command === 'start_terminal_task')).toBe(false);
});


test('synthetic archive: cursor pages replace display cache without model submission', async ({ page }) => {
  await fixture(page, true);
  await page.getByRole('button', { name: '关闭上下文' }).click();
  await page.getByRole('button', { name: '翻阅已保存对话' }).click();
  const log = page.getByRole('log', { name: '归档历史页' });
  await expect(log.getByRole('article')).toHaveCount(64);
  await expect(log).toContainText('归档消息 130');
  await page.getByRole('button', { name: '更早一页' }).click();
  await expect(log.getByRole('article')).toHaveCount(64);
  await expect(log).not.toContainText('归档消息 130');
  await page.getByRole('button', { name: '更早一页' }).click();
  await expect(log.getByRole('article')).toHaveCount(2);
  await expect(page.getByRole('button', { name: '更早一页' })).toBeDisabled();
  await page.getByRole('button', { name: '最新一页' }).click();
  await expect(log.getByRole('article')).toHaveCount(64);
  await expect(log).toContainText('归档消息 130');
  const calls = await page.evaluate(() => (window as unknown as { __CONTEXT_CALLS__: Array<{ command: string; args: Record<string, unknown> }> }).__CONTEXT_CALLS__);
  expect(calls.filter(call => call.command === 'read_saved_chat_page').map(call => call.args.beforeSequence)).toEqual([null, 67, 3, null]);
  expect(calls.some(call => ['submit_conversation_turn', 'start_terminal_task', 'compact_context'].includes(call.command))).toBe(false);
  await page.getByRole('button', { name: '收起历史页' }).click();
  await expect(log).toBeHidden();
});
