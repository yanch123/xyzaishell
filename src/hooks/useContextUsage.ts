import { useCallback, useEffect, useRef, useState } from 'react';
import { contextBridge } from '../lib/contextBridge';
import { activeCompaction, type CompactionJobStatus, type CompactionMode, type ContextUsageSnapshot } from '../lib/context';
import { newerRevision } from '../lib/model';
import { safeError } from '../lib/types';

export function useContextUsage(chatId: string, chatRevision: string, configRevision: string | null, draft: string, enabled: boolean) {
  const [snapshot, setSnapshot] = useState<ContextUsageSnapshot | null>(null);
  const [committed, setCommitted] = useState<ContextUsageSnapshot | null>(null);
  const [job, setJob] = useState<CompactionJobStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [uncertain, setUncertain] = useState(false);
  const current = useRef({ chatId, configRevision, draft }); current.current = { chatId, configRevision, draft };
  const snapshotRef = useRef<ContextUsageSnapshot | null>(null);
  const revisionFloor = useRef('0');
  const jobRef = useRef<CompactionJobStatus | null>(null);
  const request = useRef(0);
  const writing = useRef(false);
  const mounted = useRef(true);
  const acceptUsage = useCallback((next: ContextUsageSnapshot, preview = false) => {
    if (!mounted.current || next.chat_id !== current.current.chatId || next.config_revision !== current.current.configRevision) return;
    if (newerRevision(revisionFloor.current, next.context_revision)) return;
    revisionFloor.current = next.context_revision;
    if (next.measurement_scope === 'committed_context') setCommitted(next);
    if (!preview && current.current.draft) return;
    snapshotRef.current = next; setSnapshot(next);
  }, []);
  const acceptJob = useCallback((next: CompactionJobStatus | null) => {
    if (!mounted.current || !next || next.chat_id !== current.current.chatId) return;
    const old = jobRef.current;
    if (old && newerRevision(old.revision, next.revision)) return;
    if (old?.job_id === next.job_id && old.revision === next.revision && !activeCompaction(old) && activeCompaction(next)) return;
    jobRef.current = next; setJob(next);
  }, []);
  const refresh = useCallback(async () => {
    if (!enabled) return;
    const serial = ++request.current;
    const identity = current.current;
    try {
      const [usage, latestJob] = await Promise.all([contextBridge.usage(chatId, identity.draft), contextBridge.job(chatId)]);
      if (!mounted.current || serial !== request.current || identity !== current.current && (identity.draft !== current.current.draft || identity.configRevision !== current.current.configRevision)) return;
      acceptUsage(usage, true); acceptJob(latestJob); setError(null); setUncertain(false);
    } catch (failure) { if (mounted.current && serial === request.current) setError(safeError(failure)); }
  }, [enabled, chatId, acceptUsage, acceptJob]);
  useEffect(() => {
    mounted.current = true;
    if (!enabled) return () => { mounted.current = false; };
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void contextBridge.listen(event => {
      if (disposed) return;
      if (event.kind === 'context_usage_updated') { acceptUsage(event.usage); if (current.current.draft) void refresh(); }
      if (event.kind === 'context_compaction_updated') { acceptJob(event.job); if (!activeCompaction(event.job)) void refresh(); }
    }).then(stop => { if (disposed) stop(); else unlisten = stop; }).catch(failure => { if (!disposed) setError(safeError(failure)); });
    const timer = window.setInterval(() => void refresh(), 4000);
    const visible = () => { if (document.visibilityState === 'visible') void refresh(); };
    document.addEventListener('visibilitychange', visible);
    return () => { disposed = true; mounted.current = false; ++request.current; unlisten?.(); window.clearInterval(timer); document.removeEventListener('visibilitychange', visible); };
  }, [enabled, acceptUsage, acceptJob, refresh]);
  useEffect(() => {
    // Invalidate queued preview responses immediately on every input/config change.
    ++request.current;
    if (snapshotRef.current?.config_revision !== configRevision) { snapshotRef.current = null; revisionFloor.current = '0'; setSnapshot(null); setCommitted(null); }
    const timer = window.setTimeout(() => void refresh(), 180);
    return () => window.clearTimeout(timer);
  }, [draft, chatRevision, configRevision, refresh]);
  const compact = async (mode: CompactionMode) => {
    const usage = snapshotRef.current;
    if (!enabled || writing.current || uncertain || activeCompaction(jobRef.current) || !usage || usage.status !== 'ready') return;
    writing.current = true; setPending(true); setError(null);
    try { acceptJob(await contextBridge.compact(chatId, usage.context_revision, mode, crypto.randomUUID())); }
    catch (failure) { if (mounted.current) { setError(safeError(failure)); setUncertain(true); } }
    finally { writing.current = false; if (mounted.current) setPending(false); }
  };
  const cancel = async () => {
    const active = jobRef.current;
    if (!enabled || writing.current || !active || !activeCompaction(active)) return;
    writing.current = true; setPending(true);
    try { acceptJob(await contextBridge.cancel(chatId, active.job_id)); }
    catch (failure) { if (mounted.current) { setError(safeError(failure)); setUncertain(true); } }
    finally { writing.current = false; if (mounted.current) setPending(false); }
  };
  return { snapshot, committed, job, error, pending, uncertain, compact, cancel, refresh };
}
