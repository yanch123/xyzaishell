import type { ApiError, SessionRef } from './types';

/** All execution authority lives in the native dispatcher; these are display DTOs. */
export type TaskPhase = 'preparing' | 'awaiting_display' | 'waiting_approval' | 'dispatching' | 'awaiting_completion' | 'unsupported_action' | 'needs_user' | 'needs_secrets' | 'completed' | 'cancelled' | 'failed';
export type TerminalAction = {
  kind?: 'command' | 'secret_input';
  secret_field?: string | null;
  action_id: string;
  display_revision: string;
  command_text: string;
  append_enter: true;
  digest: string;
  policy: 'auto_read_only' | 'auto_normal' | 'require_approval' | 'manual_only';
  risk?: 'normal' | 'dangerous' | 'unknown';
  blocked?: boolean;
  exit_code?: number | null;
  command_completed?: boolean;
  reason: string;
  approval_id: string | null;
  expires_at_unix_ms: number;
  displayed: boolean;
  delivery: 'not_sent' | 'queued' | 'dispatch_started' | 'write_accepted' | 'unknown';
};
export type TerminalTaskSnapshot = {
  chat_id: string;
  task_id: string;
  run_id: string;
  session: SessionRef;
  target: { host: string; port: number; username: string };
  revision: string;
  phase: TaskPhase;
  instruction: string;
  assistant_text: string;
  action: TerminalAction | null;
  actions: TerminalAction[];
  terminal_ready: boolean;
  error: ApiError | null;
  owner: boolean;
  remote_outcome: 'unknown' | 'not_sent' | 'foreground_returned';
  round: number;
  secret_request?: { fields: { name: string; required: boolean }[]; purpose: string } | null;
};
export type StartTerminalTask = {
  chat_id: string;
  session: SessionRef;
  instruction: string;
  readiness_confirmed: true;
  context_consent: { revision: string; destination: string };
};
export type TaskIdentity = { task_id: string; run_id: string };
export type ActionIdentity = TaskIdentity & { action_id: string; display_revision: string };
export type AgentEvent = { kind: 'task_updated'; task: TerminalTaskSnapshot };
export const TASK_PHASE_LABELS: Record<TaskPhase, string> = {
  preparing: '模型分析中', awaiting_display: '命令已准备 · 等待显示', waiting_approval: '等待本次批准',
  awaiting_completion: '等待命令返回与 Shell 就绪', unsupported_action: '未知风险 · 需要手动处理',
  needs_secrets: '需要任务秘密 · 等待本机填写', dispatching: '正在派发', needs_user: '需你核实终端', completed: 'AI 流程已结束', cancelled: 'AI 已停止', failed: 'AI 流程出错',
};
export const DELIVERY_LABELS: Record<TerminalAction['delivery'], string> = {
  not_sent: '尚未发送', queued: '已排队 · 尚未写入', dispatch_started: '已开始写入 · 结果未知',
  write_accepted: 'SSH 接受写入 · 不代表命令成功', unknown: '发送情况未知 · 不会自动重发',
};
/** Reveal invisible controls instead of allowing misleading display order. */
export function visibleCommand(text: string): string {
  return Array.from(text, character => {
    const code = character.codePointAt(0)!;
    return code < 0x20 || (code >= 0x7f && code <= 0x9f) || [0x061c, 0x200b, 0x200c, 0x200d, 0x200e, 0x200f, 0x2028, 0x2029, 0x2060, 0xfeff].includes(code) || (code >= 0x202a && code <= 0x202e) || (code >= 0x2066 && code <= 0x2069)
      ? `\\u{${code.toString(16).toUpperCase().padStart(4, '0')}}` : character;
  }).join('');
}
export function actionIdentity(task: TerminalTaskSnapshot): ActionIdentity | null {
  return task.action ? { task_id: task.task_id, run_id: task.run_id, action_id: task.action.action_id, display_revision: task.action.display_revision } : null;
}
