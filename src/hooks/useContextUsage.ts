import { useCallback, useEffect, useRef, useState } from 'react';
import { contextBridge } from '../lib/contextBridge';
import { activeCompaction, type CompactionJobStatus, type CompactionMode, type ContextUsageSnapshot } from '../lib/context';
import { newerRevision } from '../lib/model';
import { safeError } from '../lib/types';

export function useContextUsage(chatId: string, chatRevision: string, configRevision: string | null, draft: string, enabled: boolean) {
  const [snapshot, setSnapshot] = useState<ContextUsageSnapshot | null>(null);
  const [committed, setCommitted] = useState<ContextUsageSnapshot | null>(null);
  const [job, setJob] = useState<CompactionJobStatus | null>(null);
  const [readError, setReadError] = useState<string | null>(null);
  const [operationError, setOperationError] = useState<string | null>(null);
  const [measuredIdentity, setMeasuredIdentity] = useState<string | null>(null);
  const identityKey = JSON.stringify([chatId, chatRevision, configRevision, draft]);
  const measuredIdentityRef = useRef<string | null>(null);
  const uncertainJob = useRef<string | null>(null);
  const uncertainCancellation = useRef(false);
  const [pending, setPending] = useState(false);
  const [uncertain, setUncertain] = useState(false);
  const current = useRef({ chatId, chatRevision, configRevision, draft, identityKey }); current.current = { chatId, chatRevision, configRevision, draft, identityKey };
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
    if (uncertainJob.current === next.job_id && (!uncertainCancellation.current || !activeCompaction(next))) { uncertainJob.current = null; uncertainCancellation.current = false; setUncertain(false); setOperationError(null); }
  }, []);
  const refresh = useCallback(async () => {
    if (!enabled) return;
    const serial = ++request.current;
    const identity = current.current;
    try {
      const [usage, latestJob] = await Promise.all([contextBridge.usage(chatId, identity.draft), contextBridge.job(chatId)]);
      if (!mounted.current || serial !== request.current || identity.identityKey !== current.current.identityKey) return;
      acceptUsage(usage, true); acceptJob(latestJob); setReadError(null);
      if (snapshotRef.current === usage) { measuredIdentityRef.current = identity.identityKey; setMeasuredIdentity(identity.identityKey); }
    } catch (failure) { if (mounted.current && serial === request.current) setReadError(safeError(failure)); }
  }, [enabled, chatId, acceptUsage, acceptJob]);
  useEffect(() => {
    mounted.current = true;
    if (!enabled) return () => { mounted.current = false; };
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void contextBridge.listen(event => {
      if (disposed) return;
      if (event.kind === 'context_usage_updated') { acceptUsage(event.usage); void refresh(); }
      if (event.kind === 'context_compaction_updated') { acceptJob(event.job); if (!activeCompaction(event.job)) void refresh(); }
    }).then(stop => { if (disposed) stop(); else unlisten = stop; }).catch(failure => { if (!disposed) setReadError(safeError(failure)); });
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
    if (!enabled || writing.current || uncertain || measuredIdentityRef.current !== current.current.identityKey || activeCompaction(jobRef.current) || !usage || usage.status !== 'ready') return;
    const clientRequestId = crypto.randomUUID();
    writing.current = true; setPending(true); setOperationError(null);
    try { acceptJob(await contextBridge.compact(chatId, usage.context_revision, mode, clientRequestId)); }
    catch (failure) {
      if (mounted.current) {
        setOperationError(safeError(failure));
        // compact_context returns typed ApiError only before admission; after a
        // job is registered it returns its receipt. Validate the full host DTO,
        // including quota/redaction errors from the preceding usage preparation.
        // Strings, JS Errors and incomplete objects remain unknown IPC outcomes.
        const hostError = typeof failure === 'object' && failure !== null ? failure as Record<string, unknown> : null;
        const rejected = !!hostError && typeof hostError.code === 'string' && /^[A-Z_]{3,64}$/.test(hostError.code)
          && typeof hostError.safe_message === 'string' && (hostError.retry === 'never' || hostError.retry === 'after_user_action');
        uncertainCancellation.current = false; uncertainJob.current = rejected ? null : clientRequestId; setUncertain(!rejected);
        measuredIdentityRef.current = null; setMeasuredIdentity(null);
        void refresh();
      }
    }
    finally { writing.current = false; if (mounted.current) setPending(false); }
  };
  const cancel = async () => {
    const active = jobRef.current;
    if (!enabled || writing.current || !active || !activeCompaction(active)) return;
    writing.current = true; setPending(true); setOperationError(null);
    try { acceptJob(await contextBridge.cancel(chatId, active.job_id)); }
    catch (failure) { if (mounted.current) { setOperationError(safeError(failure)); uncertainCancellation.current = true; uncertainJob.current = active.job_id; setUncertain(true); } }
    finally { writing.current = false; if (mounted.current) setPending(false); }
  };
  return { snapshot, committed, job, error: operationError ?? readError, operationError, fresh: measuredIdentity === identityKey, pending, uncertain, compact, cancel, refresh };
}
