import { useCallback, useEffect, useRef, useState } from 'react';
import { desktopAvailable, sshBridge } from '../lib/bridge';
import { INITIAL_SESSION, sameSession, safeError } from '../lib/types';
import type { ConnectRequest, HostKeyChallenge, SessionSnapshot, TerminalOutput, TerminalSize } from '../lib/types';

export function useSshSession() {
  const native = desktopAvailable();
  const [snapshot, setSnapshot] = useState<SessionSnapshot>(INITIAL_SESSION);
  const [ready, setReady] = useState(false);
  const [historyUnavailable, setHistoryUnavailable] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [challenge, setChallenge] = useState<HostKeyChallenge | null>(null);
  const snapshotRef = useRef(snapshot);
  const outputListeners = useRef(new Set<(output: TerminalOutput) => void>());
  const size = useRef<TerminalSize>({ cols: 100, rows: 30 });
  const operation = useRef(false);
  const alive = useRef(false);
  const acceptSnapshot = useCallback((next: SessionSnapshot) => {
    const current = snapshotRef.current;
    if (next.revision && current.revision && BigInt(next.revision) < BigInt(current.revision)) return;
    snapshotRef.current = next;
    setSnapshot(next);
    setChallenge(next.host_key ?? null);
    if (next.error) setError(safeError(next.error));
  }, []);

  useEffect(() => {
    alive.current = true;
    if (!native) return () => { alive.current = false; };
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await sshBridge.listen(event => {
          if (disposed) return;
          if (event.type === 'session.state') acceptSnapshot(event.snapshot);
          else if (event.type === 'host_key.challenge' && sameSession(event.challenge.session, snapshotRef.current.session)) setChallenge(event.challenge);
          else if (event.type === 'terminal.output' && sameSession(event.session, snapshotRef.current.session)) outputListeners.current.forEach(fn => fn(event));
          else if (event.type === 'session.exit' && sameSession(event.session, snapshotRef.current.session)) {
            setError(`远端 Shell 已退出（状态码 ${event.exit_status}）。这不代表此前每条命令均成功。`);
          }
        });
        if (disposed) { unlisten(); return; }
        const current = await sshBridge.snapshot();
        if (!disposed) { acceptSnapshot(current); setHistoryUnavailable(current.state === 'connected'); setReady(true); }
      } catch (failure) {
        if (!disposed) setError(safeError(failure));
      }
    })();
    return () => { alive.current = false; disposed = true; unlisten?.(); };
  }, [native, acceptSnapshot]);

  const connect = async (request: ConnectRequest): Promise<boolean> => {
    if (!ready || operation.current) return false;
    operation.current = true;
    setError(null); setHistoryUnavailable(false);
    try { const next = await sshBridge.connect(request); if (alive.current) acceptSnapshot(next); return true; }
    catch (failure) { if (alive.current) setError(safeError(failure)); return false; }
    finally { operation.current = false; }
  };
  const disconnect = async (): Promise<void> => {
    const session = snapshotRef.current.session;
    if (!session || operation.current) return;
    operation.current = true;
    try { await sshBridge.disconnect(session); }
    catch (failure) { if (alive.current) setError(safeError(failure)); }
    finally { operation.current = false; }
  };
  const confirmHostKey = async (item: HostKeyChallenge, accept: boolean): Promise<void> => {
    try {
      await sshBridge.confirmHostKey(item, accept);
      if (alive.current) setChallenge(current => current?.challenge_id === item.challenge_id ? null : current);
    } catch (failure) { if (alive.current) setError(safeError(failure)); throw failure; }
  };
  const subscribeOutput = useCallback((fn: (output: TerminalOutput) => void) => {
    outputListeners.current.add(fn);
    return () => { outputListeners.current.delete(fn); };
  }, []);
  const reportError = useCallback((failure: unknown) => setError(safeError(failure)), []);
  return { native, ready, historyUnavailable, snapshot, error, setError, challenge, connect, disconnect, confirmHostKey, subscribeOutput, reportError, size };
}
