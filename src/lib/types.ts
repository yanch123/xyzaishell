/** M1 DTOs shared with the native host. Secrets only enter ConnectRequest. */
export type SessionRef = { session_id: string; generation: string };
export type TerminalSize = { cols: number; rows: number };
export type SessionState = 'disconnected' | 'connecting' | 'awaiting_host_key' | 'authenticating' | 'connected' | 'closing' | 'error';
export type ApiError = { code: string; safe_message: string; retry: 'never' | 'after_user_action' };
export type HostKeyChallenge = {
  challenge_id: string; session: SessionRef; host: string; port: number;
  algorithm: string; fingerprint: string; previous_fingerprint?: string; changed: boolean;
};
export type SessionSnapshot = {
  session: SessionRef | null; state: SessionState; revision?: string; host?: string; port?: number;
  username?: string; error?: ApiError; host_key?: HostKeyChallenge;
};
export type ConnectRequest = {
  host: string; port: number; username: string; session_only: true; size: TerminalSize;
  auth: { kind: 'password'; password: string } | { kind: 'private_key'; private_key_path: string; passphrase?: string };
};
export type TerminalOutput = { type: 'terminal.output'; session: SessionRef; seq: string; data_base64: string; stream: 'stdout' | 'stderr' };
export type SshEvent =
  | { type: 'session.state'; snapshot: SessionSnapshot }
  | { type: 'host_key.challenge'; challenge: HostKeyChallenge }
  | TerminalOutput
  | { type: 'session.exit'; session: SessionRef; exit_status: number };
export const INITIAL_SESSION: SessionSnapshot = { session: null, state: 'disconnected' };
export const SESSION_LABELS: Record<SessionState, string> = {
  disconnected: '未连接', connecting: '正在连接', awaiting_host_key: '等待身份核验',
  authenticating: '正在认证', connected: '已连接', closing: '正在断开', error: '连接失败',
};
export function sameSession(a: SessionRef | null | undefined, b: SessionRef | null | undefined): boolean {
  return !!a && !!b && a.session_id === b.session_id && a.generation === b.generation;
}
export function isActiveSession(state: SessionState): boolean {
  return state !== 'disconnected' && state !== 'error';
}
export function safeError(error: unknown): string {
  // Never display raw thrown strings: a third-party error can contain credentials.
  if (typeof error === 'object' && error !== null && 'safe_message' in error && 'code' in error) {
    const candidate = error as Partial<ApiError>;
    if (typeof candidate.code === 'string' && /^[A-Z_]{3,64}$/.test(candidate.code) && typeof candidate.safe_message === 'string') {
      return candidate.safe_message.slice(0,500);
    }
  }
  return '操作未能完成。为避免重复执行，未自动重试。请检查连接状态后再操作。';
}
