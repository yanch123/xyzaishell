import type { TerminalTaskSnapshot } from './agent';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { ChatSnapshot, ModelConfigStatus, ModelEvent, SaveModelConfigRequest, ModelTestRequest, ModelTestResult } from './model';

import type { SessionRef } from './types';

export type ConversationTurn = { kind: 'chat'; chat: ChatSnapshot } | { kind: 'terminal'; task: TerminalTaskSnapshot };
export const modelBridge = {
  probe: () => invoke<ChatSnapshot>('probe_model_compatibility'),
  config: () => invoke<ModelConfigStatus>('get_model_config_status'),
  reload: () => invoke<ModelConfigStatus>('reload_model_config'),
  save: (request: SaveModelConfigRequest) => invoke<ModelConfigStatus>('save_model_config', { request }),
  testSaved: (request: ModelTestRequest) => invoke<ModelTestResult>('test_model_config', { request }),
  cancelTest: (testId: string) => invoke<void>('cancel_model_config_test', { testId }),
  confirm: (revision: string, destination: string) => invoke<ModelConfigStatus>('confirm_model_destination', { revision, destination }),
  list: () => invoke<ChatSnapshot[]>('list_chats'),
  create: (title = '新任务', source: SessionRef | null = null) => invoke<ChatSnapshot>('create_chat', { title, source }),
  get: (chatId: string) => invoke<ChatSnapshot>('get_chat', { chatId }),
  send: (chatId: string, text: string, revision: string, destination: string, session: SessionRef | null, submissionId: string) => invoke<ConversationTurn>('submit_conversation_turn', { chatId, text, revision, destination, session, submissionId }),
  confirmScope: (revision: string, destination: string, session: SessionRef | null) => invoke<ModelConfigStatus>('confirm_conversation_scope', { request: { revision, destination, session } }),
  submission: (submissionId: string) => invoke<ConversationTurn | null>('get_conversation_submission', { submissionId }),
  sendText: (chatId: string, text: string) => invoke<ChatSnapshot>('send_chat_message', { chatId, text }),
  cancel: (chatId: string, runId: string) => invoke<ChatSnapshot>('cancel_chat_run', { chatId, runId }),
  listen: (handler: (event: ModelEvent) => void) => listen<ModelEvent>('model:event', event => handler(event.payload)),
};
