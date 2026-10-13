import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { ConnectRequest, HostKeyChallenge, SessionRef, SessionSnapshot, SshEvent, TerminalSize } from './types';

export const desktopAvailable = (): boolean => isTauri();
export const sshBridge = {
  snapshot: () => invoke<SessionSnapshot>('get_session_snapshot'),
  connect: (request: ConnectRequest) => invoke<SessionSnapshot>('connect_server', { request }),
  disconnect: (session: SessionRef) => invoke('disconnect_server', { session }),
  resize: (session: SessionRef, size: TerminalSize) => invoke('resize_terminal', { session, size }),
  ackOutput: (session: SessionRef, seq: string) => invoke('ack_terminal_output', { session, seq }),
  write: (session: SessionRef, dataBase64: string) => invoke('send_user_input', { session, dataBase64 }),
  confirmHostKey: (challenge: HostKeyChallenge, accept: boolean) => invoke('confirm_host_key', { request: {
    challenge_id: challenge.challenge_id, expected_fingerprint: challenge.fingerprint, decision: accept ? 'accept' : 'reject',
  } }),
  listen: (handler: (event: SshEvent) => void) => listen<SshEvent>('ssh:event', event => handler(event.payload)),
};
