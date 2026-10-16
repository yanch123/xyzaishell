import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { CompactionJobStatus, CompactionMode, ContextUsageSnapshot } from './context';
export type ContextEvent = { kind: 'context_usage_updated'; usage: ContextUsageSnapshot } | { kind: 'context_compaction_updated'; job: CompactionJobStatus };
/** Preview is local IPC only. No draft is passed to a compaction operation. */
export const contextBridge = {
  usage: (chatId: string, draft = '') => invoke<ContextUsageSnapshot>('get_context_usage', { chatId, draft }),
  job: (chatId: string) => invoke<CompactionJobStatus | null>('get_context_compaction', { chatId }),
  compact: (chatId: string, expectedContextRevision: string, mode: CompactionMode, clientRequestId: string) => invoke<CompactionJobStatus>('compact_context', { chatId, expectedContextRevision, mode, clientRequestId }),
  cancel: (chatId: string, jobId: string) => invoke<CompactionJobStatus>('cancel_context_compaction', { chatId, jobId }),
  listen: (handler: (event: ContextEvent) => void) => listen<ContextEvent>('model:event', event => handler(event.payload)),
};
