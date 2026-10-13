import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { ChatSnapshot, ModelConfigStatus, ModelEvent } from './model';

export const modelBridge = {
  probe: () => invoke<ChatSnapshot>('probe_model_compatibility'),
  config: () => invoke<ModelConfigStatus>('get_model_config_status'),
  reload: () => invoke<ModelConfigStatus>('reload_model_config'),
  confirm: (revision: string, destination: string) => invoke<ModelConfigStatus>('confirm_model_destination', { revision, destination }),
  list: () => invoke<ChatSnapshot[]>('list_chats'),
  create: (title = '新任务') => invoke<ChatSnapshot>('create_chat', { title }),
  get: (chatId: string) => invoke<ChatSnapshot>('get_chat', { chatId }),
  send: (chatId: string, text: string) => invoke<ChatSnapshot>('send_chat_message', { chatId, text }),
  cancel: (chatId: string, runId: string) => invoke<ChatSnapshot>('cancel_chat_run', { chatId, runId }),
  listen: (handler: (event: ModelEvent) => void) => listen<ModelEvent>('model:event', event => handler(event.payload)),
};
