import { useEffect, useRef, useState } from 'react';
import { integrationBridge } from '../lib/integrationBridge';
import type { IntegrationStatus } from '../lib/integrationBridge';
import { safeError, sameSession } from '../lib/types';
import type { SessionSnapshot } from '../lib/types';

export function useShellIntegration(native: boolean, snapshot: SessionSnapshot) {
  const [status, setStatus] = useState<IntegrationStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [uncertain, setUncertain] = useState(false);
  const lock = useRef(false); const epoch = useRef(0); const current = useRef(snapshot); current.current = snapshot;
  const statusRef = useRef<IntegrationStatus | null>(null);
  const accept = (next: IntegrationStatus) => {
    if (next.session && !sameSession(next.session, current.current.session)) return;
    if (statusRef.current && next.revision < statusRef.current.revision) return;
    statusRef.current = next; setStatus(next);
  };
  useEffect(() => {
    const life = ++epoch.current; let stop = false; let timer: ReturnType<typeof setTimeout>;
    statusRef.current = null; lock.current = false; setBusy(false); setStatus(null); setError(null); setUncertain(false);
    if (!native || snapshot.state !== 'connected') return;
    const read = async () => {
      try { const next = await integrationBridge.status(); if (!stop && epoch.current === life) accept(next); }
      catch (failure) { if (!stop && epoch.current === life) setError(safeError(failure)); }
      if (!stop) timer = setTimeout(() => void read(), 2000);
    };
    void read();
    return () => { stop = true; epoch.current++; clearTimeout(timer); };
  }, [native, snapshot.session?.session_id, snapshot.session?.generation, snapshot.state]);
  const install = async () => {
    const source = current.current;
    if (!native || !source.session || source.state !== 'connected' || lock.current || uncertain) return;
    lock.current = true; setBusy(true); setError(null); const life = epoch.current;
    try { const next = await integrationBridge.install(source.session); if (life === epoch.current) accept(next); }
    catch (failure) { if (life === epoch.current) { setError(safeError(failure)); setUncertain(true); } }
    finally { if (life === epoch.current) { lock.current = false; setBusy(false); } }
  };
  return { status, busy, error, uncertain, install };
}
export type ShellIntegrationController = ReturnType<typeof useShellIntegration>;
