import type { ApiError, SessionRef } from './types';

/** Safe native DTOs. Configuration secrets never cross this boundary. */
export type ModelConfigStatus = {
  context_window_tokens?: number | null;
  context_window_override_tokens?: number | null;
  capacity_source?: 'product_default' | 'user_override' | 'verified_metadata' | 'unavailable';
  output_reserve_tokens?: number | null;
  safety_margin_tokens?: number | null;
  input_budget_tokens?: number | null;
  context_auto_compact?: boolean;
  context_auto_mode?: 'llm' | 'quick';
  path: string | null;
  revision: string | null;
  file_revision: string | null;
  base_url: string | null;
  key_present: boolean;
  path_overridden: boolean;
  ready: boolean;
  destination: string | null;
  model: string | null;
  consented: boolean;
  error: ApiError | null;
  warning: string | null;
  context_policy: 'explicit_chat_only';
  persistence: 'memory_only';
};
export type ChatMessage = {
  id: string;
  role: 'user' | 'assistant';
  text: string;
  run_id: string;
  state: 'complete' | 'streaming' | 'cancelled' | 'error';
  error: ApiError | null;
  destination?: string | null;
  model?: string | null;
};
export type ModelKeyUpdate = { kind: 'keep' } | { kind: 'replace'; value: string } | { kind: 'clear' };
export type SaveModelConfigRequest = {
  context_window_tokens?: number | null;
  context_auto_compact?: boolean;
  context_auto_mode?: 'llm' | 'quick';
  expected_file_revision: string;
  base_url: string;
  model: string;
  key_update: ModelKeyUpdate;
  reuse_key_for_destination: string | null;
};
export type ModelTestKind = 'chat_connectivity' | 'task_capability';
export type ModelTestOutcome = 'supported' | 'unsupported' | 'inconclusive';
/** Omitted kind preserves the text-only test for older callers. */
export type ModelTestRequest = { test_id: string; file_revision: string; destination: string; model: string; consent: boolean; kind?: ModelTestKind };
/** A supported synthetic probe is not approval or proof of safe task execution. */
export type ModelTestResult = { test_id: string; file_revision: string; destination: string; model: string; kind: ModelTestKind; outcome: ModelTestOutcome; success: boolean; error: ApiError | null };
export type ChatSource = SessionRef & { host: string; port: number; username: string };
export type ChatSnapshot = {
  chat_id: string;
  task_id: string;
  title: string;
  source?: ChatSource | null;
  restored_read_only?: boolean;
  messages: ChatMessage[];
  active_run_id: string | null;
  revision: string;
};
export type ModelEvent = { kind: 'chat_updated'; chat: ChatSnapshot } | { kind: 'config_updated'; config: ModelConfigStatus } | import('./contextBridge').ContextEvent;
export const INITIAL_MODEL_CONFIG: ModelConfigStatus = {
  path: null, revision: null, file_revision: null, base_url: null, key_present: false, path_overridden: false, ready: false, destination: null, model: null,
  consented: false, error: null, warning: null, context_policy: 'explicit_chat_only', persistence: 'memory_only',
};
export const MAX_CONCURRENT_CHATS = 4;

/** Revisions are decimal strings rather than lossy JavaScript numbers. */
export function newerRevision(incoming: string, current: string): boolean {
  return /^\d+$/.test(incoming) && /^\d+$/.test(current) && BigInt(incoming) > BigInt(current);
}
