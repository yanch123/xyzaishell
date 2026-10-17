import type { ApiError } from './types';
export type CompactionMode = 'llm' | 'quick';
export const DISPLAY_CATEGORIES = ['system_instructions', 'tool_definitions', 'fixed_rules', 'history_summary', 'current_conversation'] as const;
export const DIALOGUE_PARTS = ['user_messages', 'assistant_messages', 'tool_calls', 'tool_results', 'other_inputs'] as const;
export type DisplayCategory = typeof DISPLAY_CATEGORIES[number];
export type DialoguePart = typeof DIALOGUE_PARTS[number];
export type ContentEstimate = { category: DisplayCategory; estimated_tokens: number | null; children?: { category: DialoguePart; estimated_tokens: number | null }[] };
export type ContextEstimate = {
  method: 'model_tokenizer' | 'heuristic' | 'unavailable'; estimator_version: string; allocation_method: 'token_spans' | 'normalized_estimate'; estimated_input_tokens: number | null; conservative_input_tokens: number | null;
  category_schema_version: 2; categories: ContentEstimate[]; content_input_tokens: number | null;
  protocol_overhead: { method: 'protocol_structure_v1'; known_framing_tokens: number | null; template_allowance_tokens: number | null; estimated_tokens: number | null };
};
export type ProviderUsage = { request_id: string; request_kind: 'conversation' | 'planning' | 'compaction'; projection_id: string; config_revision: string; model: string; received_at: string; input_tokens: number | null; output_tokens: number | null; cached_input_tokens: number | null; reasoning_tokens: number | null; completeness: 'reported' | 'partial' };
export type ContextUsageSnapshot = {
  chat_id: string; context_revision: string; config_revision: string; projection_id: string; request_kind: 'conversation' | 'planning' | 'compaction'; turn_id: string; turn_phase: 'end_of_turn' | 'active_turn'; measurement_scope: 'committed_context' | 'draft_preview'; prepared_at: string; status: 'ready' | 'estimating' | 'unavailable' | 'stale';
  estimate: ContextEstimate;
  budget: { context_window_tokens: number | null; capacity_source: 'verified_metadata' | 'user_override' | 'product_default' | 'unavailable'; output_reserve_tokens: number; safety_margin_tokens: number | null; input_budget_tokens: number | null; decision_input_tokens: number | null; input_occupancy_ratio: number | null; after_turn_trigger_ratio: number; in_turn_trigger_ratio: number; effective_trigger_ratio: number };
  byte_guards: { scope: 'planning_prompt' | 'serialized_request'; measured_bytes: number; limit_bytes: number; exceeded: boolean }[];
  last_request_usage: ProviderUsage | null; last_compaction_usage: ProviderUsage | null;
};
export type CompactionJobStatus = { job_id: string; chat_id: string; revision: string; context_revision?: string; state: 'idle' | 'requested' | 'waiting_safe_point' | 'snapshotting' | 'estimating' | 'compressing' | 'reducing' | 'validating' | 'committing' | 'succeeded' | 'cancelled' | 'stale' | 'failed' | 'no_effect'; mode: CompactionMode; reason: 'manual' | 'token_pressure' | 'hard_byte_limit' | 'both'; error?: ApiError | null; before_request_bytes?: number | null; after_request_bytes?: number | null; before_input_tokens?: number | null; after_input_tokens?: number | null };
export const activeCompaction = (job: CompactionJobStatus | null) => !!job && ['requested', 'waiting_safe_point', 'snapshotting', 'estimating', 'compressing', 'reducing', 'validating', 'committing'].includes(job.state);
export function occupancy(snapshot: ContextUsageSnapshot | null): string {
  const value = snapshot?.budget.input_occupancy_ratio;
  return value == null || !Number.isFinite(value) || value < 0 || !snapshot?.budget.input_budget_tokens || snapshot.status === 'unavailable' ? '—' : `≈${Math.round(value * 100)}%`;
}
export const tokenLabel = (value: number | null | undefined) => value == null ? '未知' : value >= 1000 ? `${Number((value / 1000).toFixed(1))}k` : String(value);
