import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { ActionIdentity, AgentEvent, StartTerminalTask, TaskIdentity, TerminalTaskSnapshot } from './agent';

export const agentBridge = {
  prepareSecret: (request: { task_id: string; run_id: string; field_name: string; purpose: string; prompt_confirmed: true }) => invoke<TerminalTaskSnapshot>('prepare_secret_input', { request }),
  list: () => invoke<TerminalTaskSnapshot[]>('list_terminal_tasks'),
  get: (taskId: string) => invoke<TerminalTaskSnapshot | null>('get_terminal_task', { taskId }),
  start: (request: StartTerminalTask) => invoke<TerminalTaskSnapshot>('start_terminal_task', { request }),
  ack: (request: ActionIdentity) => invoke<TerminalTaskSnapshot>('ack_terminal_action', { request }),
  decide: (request: TaskIdentity & { action_id: string; approval_id: string; decision: 'approve' | 'deny' }) => invoke<TerminalTaskSnapshot>('decide_terminal_action', { request }),
  cancel: (request: TaskIdentity) => invoke<TerminalTaskSnapshot>('cancel_terminal_task', { request }),
  continue: (request: TaskIdentity & { readiness_confirmed: true }) => invoke<TerminalTaskSnapshot>('continue_terminal_task', { request }),
  listen: (handler: (event: AgentEvent) => void) => listen<AgentEvent>('agent:event', event => handler(event.payload)),
};
