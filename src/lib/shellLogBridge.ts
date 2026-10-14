import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { ApiError, SessionRef } from './types';

/** Passive local shell records, never execution instructions or model context. */
export type CommandStatus = 'running' | 'completed' | 'unknown';
export type CaptureState = 'disabled' | 'starting' | 'recording' | 'paused' | 'gap' | 'disconnected';
export type ExportFormat = 'json' | 'csv';
export type ShellLogEntry = {
  id: string; sequence: string; session_id: string; generation: string; shell_id: string;
  shell_sequence: string; host: string; port: number; username: string;
  started_at_ms: number; finished_at_ms: number | null; remote_started_at: string | null;
  command: string; cwd: string; status: CommandStatus; exit_code: number | null; redacted: boolean;
};
export type ShellLogQuery = {
  session_id?: string | null; from_ms?: number | null; to_ms?: number | null;
  contains?: string | null; before_seq?: string | null; snapshot_max_seq?: string | null; limit?: number | null;
};
export type ShellLogPage = { entries: ShellLogEntry[]; next_before_seq: string | null; snapshot_max_seq: string };
export type ShellLogExport = { path: string; records: number; bytes: number; format: ExportFormat };
export type ShellLogStatus = {
  enabled: boolean; available: boolean; plaintext: boolean; revision: string; state: CaptureState;
  session: SessionRef | null; reason: string | null; error: ApiError | null;
};
export const shellLogBridge = {
  status: () => invoke<ShellLogStatus>('get_shell_log_status'),
  start: (session: SessionRef) => invoke<ShellLogStatus>('start_shell_log', { session, consent: true, readinessConfirmed: true }),
  pause: (paused: boolean) => invoke<ShellLogStatus>('set_shell_log_paused', { paused }),
  query: (query: ShellLogQuery) => invoke<ShellLogPage>('query_shell_log', { query }),
  export: (query: ShellLogQuery, format: ExportFormat) => invoke<ShellLogExport>('export_shell_log', { query, format }),
  listen: (handler: (status: ShellLogStatus) => void) => listen<ShellLogStatus>('shell-log:status', event => handler(event.payload)),
};
