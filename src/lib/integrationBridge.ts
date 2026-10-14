import { invoke } from '@tauri-apps/api/core';
import type { ApiError, SessionRef } from './types';
export type IntegrationPhase = 'unintegrated' | 'installing' | 'awaiting_ready' | 'ready' | 'awaiting_begin' | 'running' | 'unknown' | 'unsupported' | 'disconnected';
export type IntegrationStatus = { session?: SessionRef | null; shell_id?: string | null; phase: IntegrationPhase; sequence: number; revision: number; active_action_id?: string | null; completed?: {action_id: string; sequence: number; exit_code: number} | null; error?: ApiError | null; shadowed_commands: string[] };
export const integrationBridge = {
  status: () => invoke<IntegrationStatus>('get_shell_integration_status'),
  install: (session: SessionRef) => invoke<IntegrationStatus>('install_shell_integration', { session, readinessConfirmed: true }),
};
