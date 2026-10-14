import type { ChatSnapshot, ModelConfigStatus } from '../lib/model';
export const modelConfig: ModelConfigStatus = {
  path: '/home/test/.config/xyzaishell/model.toml', revision: '1', file_revision: 'saved-file-1', base_url: 'https://models.example.test/v1', key_present: true, path_overridden: false, ready: true,
  destination: 'https://models.example.test/v1', model: 'test-model', consented: true,
  error: null, warning: null, context_policy: 'explicit_chat_only', persistence: 'memory_only',
};
export function chat(id = 'chat-a', revision = '1'): ChatSnapshot {
  return { chat_id: id, task_id: id, title: '新任务', messages: [], active_run_id: null, revision };
}
export function stream(id = 'chat-a', revision = '2', text = ''): ChatSnapshot {
  return { ...chat(id, revision), title: `任务 ${id}`, active_run_id: `run-${id}`,
    messages: [
      { id: `user-${id}`, role: 'user', text: '解释这条命令', run_id: `run-${id}`, state: 'complete', error: null },
      { id: `answer-${id}`, role: 'assistant', text, run_id: `run-${id}`, state: 'streaming', error: null, destination: modelConfig.destination!, model: modelConfig.model! },
    ],
  };
}
