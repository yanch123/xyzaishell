import type { StartTerminalTask, TerminalAction, TerminalTaskSnapshot } from '../lib/agent';
import { actionIdentity } from '../lib/agent';
import type { SessionSnapshot } from '../lib/types';
import type { TerminalAgentController } from '../hooks/useTerminalAgent';
import { modelConfig } from './modelFixtures';
export const connectedSession: SessionSnapshot = { session: { session_id: 'ssh-a', generation: '1' }, state: 'connected', host: 'server.example.test', username: 'test', port: 22 };
export function task(overrides: Partial<TerminalTaskSnapshot> = {}): TerminalTaskSnapshot {
  return { chat_id: 'chat-a', task_id: 'chat-a', run_id: 'run-a', session: connectedSession.session!, target: { host: connectedSession.host!, username: connectedSession.username!, port: 22 }, revision: '1', phase: 'preparing', instruction: '检查服务器状态', assistant_text: '', action: null, actions: [], terminal_ready: true, error: null, owner: true, remote_outcome: 'not_sent', round: 1, ...overrides };
}
export function action(overrides: Partial<TerminalAction> = {}): TerminalAction {
  return { action_id: 'action-a', display_revision: '1', command_text: 'uname -a', append_enter: true, digest: 'opaque-fingerprint', policy: 'auto_read_only', reason: '基础策略允许的检查。', approval_id: null, expires_at_unix_ms: Date.now() + 120000, displayed: false, delivery: 'not_sent', ...overrides };
}
export function startRequest(overrides: Partial<StartTerminalTask> = {}): StartTerminalTask {
  return { chat_id: 'chat-a', session: connectedSession.session!, instruction: '检查服务器状态', readiness_confirmed: true, context_consent: { revision: modelConfig.revision!, destination: modelConfig.destination! }, ...overrides };
}
export function controller(overrides: Partial<TerminalAgentController> = {}): TerminalAgentController {
  return { native: true, ready: true, tasks: {}, owner: null, blocked: false, error: null, errors: {}, pending: {}, uncertain: {}, drafts: {}, updateDraft: () => {}, start: async () => true, acknowledge: async () => {}, decide: async () => {}, cancel: async () => {}, continueTask: async () => {}, prepareSecret: async () => {}, refresh: async () => {}, retryLoad: () => {}, actionIdentity, ...overrides };
}
