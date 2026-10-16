import type { ContextUsageSnapshot, CompactionJobStatus } from '../lib/context';
export const contextUsage = (changes: Partial<ContextUsageSnapshot> = {}): ContextUsageSnapshot => ({
  chat_id: 'chat-a', context_revision: '1', config_revision: '1', projection_id: 'projection-a', request_kind: 'conversation', turn_id: 'turn-a', turn_phase: 'active_turn', measurement_scope: 'committed_context', prepared_at: '2026-10-16T18:00:00Z', status: 'ready',
  estimate: { method: 'heuristic', estimator_version: 'heuristic-v1', allocation_method: 'normalized_estimate', estimated_input_tokens: 10000, conservative_input_tokens: 12000, categories: [{ category: 'dialogue', estimated_tokens: 8000 }, { category: 'rules_tools', estimated_tokens: 2000 }] },
  budget: { context_window_tokens: 256000, capacity_source: 'product_default', output_reserve_tokens: 4096, safety_margin_tokens: 12800, input_budget_tokens: 239104, decision_input_tokens: 12000, input_occupancy_ratio: 12000 / 239104, after_turn_trigger_ratio: .8, in_turn_trigger_ratio: .95, effective_trigger_ratio: .95 },
  byte_guards: [], last_request_usage: null, last_compaction_usage: null, ...changes,
});
export const compactionJob = (changes: Partial<CompactionJobStatus> = {}): CompactionJobStatus => ({ job_id: 'job-a', chat_id: 'chat-a', revision: '1', mode: 'llm', reason: 'manual', state: 'compressing', ...changes });
