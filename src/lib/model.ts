import type { ApiError } from './types';

/** Safe native DTOs. Configuration secrets never cross this boundary. */
export type ModelConfigStatus = {
  path: string | null;
  revision: string | null;
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
export type ChatSnapshot = {
  chat_id: string;
  task_id: string;
  title: string;
  messages: ChatMessage[];
  active_run_id: string | null;
  revision: string;
};
export type ModelEvent = { kind: 'chat_updated'; chat: ChatSnapshot } | { kind: 'config_updated'; config: ModelConfigStatus };
export const INITIAL_MODEL_CONFIG: ModelConfigStatus = {
  path: null, revision: null, ready: false, destination: null, model: null,
  consented: false, error: null, warning: null, context_policy: 'explicit_chat_only', persistence: 'memory_only',
};
export const MAX_CONCURRENT_CHATS = 4;

/** Revisions are decimal strings rather than lossy JavaScript numbers. */
export function newerRevision(incoming: string, current: string): boolean {
  return /^\d+$/.test(incoming) && /^\d+$/.test(current) && BigInt(incoming) > BigInt(current);
}
