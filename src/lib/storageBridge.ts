import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { SessionSnapshot, TerminalSize } from './types';
export type SavedProfile = { id: string; label: string; host: string; port: number; username: string; auth_kind: 'password' | 'private_key'; private_key_path: string | null; credential_present: boolean };
export type ProfileStatus = { platform: string; path_warning?: string | null; configuration_root: string | null; credentials: { credential_backend: 'dpapi_current_user' | 'linux_secret_service' | 'session_only'; credential_availability: 'available' | 'locked' | 'unavailable' | 'access_denied'; persistent_credentials: boolean }; listing: { revision: string; mode: string; connections: SavedProfile[] } };
export type SaveProfile = { expected_revision: string; profile: Omit<SavedProfile, 'credential_present' | 'id'> & { id: string | null }; credential_update: { kind: 'keep' | 'clear' | 'set'; value?: string } };
export const storageBridge = {
  profiles: () => invoke<ProfileStatus>('get_profile_status'),
  saveProfile: (request: SaveProfile) => invoke<ProfileStatus>('save_connection_profile', { request }),
  sessionCredential: (connectionId: string, expectedRevision: string, value: string | null) => invoke<ProfileStatus>('set_session_credential', { connectionId, expectedRevision, value }),
  sessionMode: (expectedRevision: string) => invoke<ProfileStatus>('set_credential_mode', { expectedRevision, mode: 'session_only' }),
  connectProfile: (connectionId: string, profileRevision: string, size: TerminalSize) => invoke<SessionSnapshot>('connect_saved_profile', { connectionId, profileRevision, size }),
};

export type HistoryStatus = { enabled: boolean; manageable?: boolean; available: boolean; plaintext: true; revision: string; error: import('./types').ApiError | null };
export type HistoryEntry = { chat_id: string; task_id: string; title: string; message_count: number; revision: string; updated_ms: number; listed: boolean };
export const historyBridge = {
  listen: (handler: (status: HistoryStatus) => void) => listen<HistoryStatus>('history:event', event => handler(event.payload)),
  status: () => invoke<HistoryStatus>('get_history_status'),
  enable: (enabled: boolean) => invoke<HistoryStatus>('set_history_enabled', { enabled }),
  list: () => invoke<HistoryEntry[]>('list_saved_chats'),
  restore: (chatId: string, expectedRevision: string) => invoke<import('./model').ChatSnapshot>('restore_saved_chat', { chatId, expectedRevision }),
  forget: (chatId: string, expectedRevision: string) => invoke<HistoryStatus>('forget_saved_chat', { chatId, expectedRevision }),
};

export type TaskSecretsView = { task_id: string; snapshot_id: string | null; fields: { name: string; present: boolean }[]; local_edit_path: string };
export const taskSecretsBridge = {
  get: (task_id: string, run_id: string) => invoke<TaskSecretsView>('get_task_secrets', { request: { task_id, run_id } }),
  ensure: (task_id: string, run_id: string, fields: { name: string; required: boolean }[], expected_snapshot_id: string | null) => invoke<TaskSecretsView>('ensure_task_secrets', { request: { task_id, run_id, fields, expected_snapshot_id } }),
  reload: (task_id: string, run_id: string) => invoke<TaskSecretsView>('reload_task_secrets', { request: { task_id, run_id } }),
};
