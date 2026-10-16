import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { SessionSnapshot, TerminalSize } from './types';
export type CredentialPolicy = 'ask_on_connect' | 'system_secure';
export type SavedProfile = { id: string; label: string; host: string; port: number; username: string; auth_kind: 'password' | 'private_key'; private_key_path: string | null; credential_present: boolean; credential_policy: CredentialPolicy };
export type ProfileStatus = { platform: string; path_warning?: string | null; configuration_root: string | null; credentials: { credential_backend: 'dpapi_current_user' | 'linux_secret_service' | 'session_only'; credential_availability: 'available' | 'locked' | 'unavailable' | 'access_denied'; persistent_credentials: boolean }; listing: { revision: string; mode: string; connections: SavedProfile[] } };
export type SaveProfile = { expected_revision: string; profile: Omit<SavedProfile, 'credential_present' | 'credential_policy' | 'id'> & { id: string | null }; credential_update: { kind: 'keep' | 'clear' | 'set'; value?: string } };
export type ProfileConnectOptions = { temporaryCredential?: string; rememberCredential?: boolean };
export type ConnectSavedProfile = (id: string, revision: string, options?: ProfileConnectOptions) => Promise<boolean>;
export const storageBridge = {
  profiles: () => invoke<ProfileStatus>('get_profile_status'),
  saveProfile: (request: SaveProfile) => invoke<ProfileStatus>('save_connection_profile', { request }),
  deleteProfile: (connectionId: string, expectedRevision: string) => invoke<ProfileStatus>('delete_connection_profile', { connectionId, expectedRevision }),
  connectProfile: (connectionId: string, profileRevision: string, size: TerminalSize, options?: ProfileConnectOptions) => invoke<SessionSnapshot>('connect_saved_profile', { connectionId, profileRevision, size, temporaryCredential: options?.temporaryCredential ?? null, rememberCredential: options?.rememberCredential ?? false }),
};

export type HistoryStatus = { enabled: boolean; manageable?: boolean; available: boolean; plaintext: true; revision: string; error: import('./types').ApiError | null };
export type HistoryEntry = { chat_id: string; task_id: string; title: string; message_count: number; revision: string; updated_ms: number; listed: boolean };
export type HistoryPage = { chat: import('./model').ChatSnapshot; total_messages: number; next_before: number | null };
export const historyBridge = {
  page: (chatId: string, expectedRevision: string, beforeSequence: number | null = null, limit = 64) => invoke<HistoryPage>('read_saved_chat_page', { chatId, expectedRevision, beforeSequence, limit }),
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
