import { useCallback, useEffect, useRef, useState } from 'react';
import { desktopAvailable } from '../lib/bridge';
import { agentBridge } from '../lib/agentBridge';
import { actionIdentity } from '../lib/agent';
import type { ActionIdentity, StartTerminalTask, TerminalTaskSnapshot } from '../lib/agent';
import type { ChatSnapshot, ModelConfigStatus } from '../lib/model';
import { newerRevision } from '../lib/model';
import type { SessionSnapshot } from '../lib/types';
import { safeError, sameSession } from '../lib/types';
import { MAX_DRAFT_LENGTH, MAX_MESSAGE_BYTES, utf8Length } from '../lib/chats';

// These native start errors are returned strictly before a run is inserted or
// launched. Every other failure remains ambiguous and must not be resubmitted.
const START_PREFLIGHT_ERRORS = new Set(['TASK_INPUT_INVALID', 'MODEL_CONFIG_CHANGED', 'MODEL_CONFIG_UNAVAILABLE', 'SESSION_STALE', 'CHAT_BUSY', 'CHAT_NOT_FOUND', 'AI_OWNER_BUSY', 'TERMINAL_OUTCOME_UNKNOWN', 'TERMINAL_READINESS_REQUIRED']);
type Pending = 'starting' | 'acknowledging' | 'deciding' | 'cancelling' | 'continuing';
export function useTerminalAgent(config: ModelConfigStatus, session: SessionSnapshot) {
  const native = desktopAvailable();
  const [tasks, setTasks] = useState<Record<string, TerminalTaskSnapshot>>({});
  const [ready, setReady] = useState(!native);
  const [error, setError] = useState<string | null>(null);
  const [errors, setErrors] = useState<Record<string, string | null>>({});
  const [pending, setPending] = useState<Record<string, Pending>>({});
  const [uncertain, setUncertain] = useState<Record<string, string>>({});
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [loadAttempt, setLoadAttempt] = useState(0);
  const tasksRef = useRef(tasks);
  const pendingRef = useRef(new Map<string, Pending>());
  const uncertainRef = useRef(new Map<string, string>());
  const attempted = useRef(new Set<string>());
  const draftsRef = useRef(drafts);
  const live = useRef({ config, session, ready }); live.current = { config, session, ready };
  const alive = useRef(false);

  const accept = useCallback((next: TerminalTaskSnapshot) => {
    const old = tasksRef.current[next.task_id];
    if (old && (!newerRevision(next.revision, old.revision) || old.chat_id !== next.chat_id)) return;
    if (old && old.run_id !== next.run_id) {
      for (const key of attempted.current) if (key.includes(`:${old.run_id}`)) attempted.current.delete(key);
    }
    // Task revisions span runs. Old run callbacks cannot take over a newer run.
    tasksRef.current = { ...tasksRef.current, [next.task_id]: next };
    setTasks(tasksRef.current);
    const before = uncertainRef.current.get(next.task_id);
    if (before !== undefined && newerRevision(next.revision, before)) {
      uncertainRef.current.delete(next.task_id); setUncertain(Object.fromEntries(uncertainRef.current));
    }
  }, []);
  const refresh = useCallback(async () => {
    if (!native) return;
    try {
      const saved = await agentBridge.list();
      if (alive.current) { saved.forEach(accept); setError(null); }
    } catch (failure) { if (alive.current) setError(safeError(failure)); }
  }, [native, accept]);
  useEffect(() => {
    alive.current = true;
    if (!native) return () => { alive.current = false; };
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await agentBridge.listen(event => { if (!disposed && event.kind === 'task_updated') accept(event.task); });
        if (disposed) { unlisten(); return; }
        const saved = await agentBridge.list();
        if (disposed) return;
        saved.forEach(accept); setReady(true); setError(null);
      } catch (failure) { if (!disposed) setError(safeError(failure)); }
    })();
    return () => { disposed = true; alive.current = false; unlisten?.(); };
  }, [native, accept, loadAttempt]);
  useEffect(() => {
    if (!native || !ready) return;
    let disposed = false;
    let reading = false;
    const reconcile = async () => {
      if (disposed || reading) return;
      reading = true;
      try {
        const saved = await agentBridge.list();
        if (!disposed) saved.forEach(accept);
      } catch (failure) { if (!disposed) setError(safeError(failure)); }
      finally { reading = false; }
    };
    const timer = window.setInterval(() => void reconcile(), 4000);
    const visible = () => { if (document.visibilityState === 'visible') void reconcile(); };
    document.addEventListener('visibilitychange', visible);
    return () => { disposed = true; window.clearInterval(timer); document.removeEventListener('visibilitychange', visible); };
  }, [native, ready, accept]);
  // Reconnection never resumes or transfers an execution run. Only read state.
  useEffect(() => { if (native && ready) void refresh(); }, [native, ready, refresh, session.session?.session_id, session.session?.generation, session.state]);

  const mark = (id: string, value: Pending | null) => {
    if (value) pendingRef.current.set(id, value); else pendingRef.current.delete(id);
    if (alive.current) setPending(Object.fromEntries(pendingRef.current));
  };
  const updateDraft = (id: string, value: string) => {
    draftsRef.current = { ...draftsRef.current, [id]: value.slice(0, MAX_DRAFT_LENGTH) };
    setDrafts(draftsRef.current);
  };
  const mutate = async (id: string, key: string, kind: Pending, call: () => Promise<TerminalTaskSnapshot>): Promise<boolean> => {
    if (!native || !live.current.ready || attempted.current.has(key)) return false;
    if (kind === 'cancelling' ? pendingRef.current.get(id) === 'cancelling' : pendingRef.current.has(id) || uncertainRef.current.has(id)) return false;
    const previous = tasksRef.current[id]?.revision ?? '0';
    attempted.current.add(key); mark(id, kind); setErrors(items => ({ ...items, [id]: null }));
    try {
      const next = await call();
      if (alive.current) accept(next);
      return true;
    } catch (failure) {
      if (alive.current) {
        setErrors(items => ({ ...items, [id]: safeError(failure) }));
        const preflight = kind === 'starting' && typeof failure === 'object' && failure !== null && 'code' in failure && typeof failure.code === 'string' && START_PREFLIGHT_ERRORS.has(failure.code);
        if (!preflight) { uncertainRef.current.set(id, previous); setUncertain(Object.fromEntries(uncertainRef.current)); }
        // Never retry a mutation. Repair ambiguous IPC using authoritative reads.
        try {
          const next = await agentBridge.get(id);
          if (alive.current && next) {
            accept(next);
            if (newerRevision(next.revision, previous)) {
              uncertainRef.current.delete(id); setUncertain(Object.fromEntries(uncertainRef.current));
            }
          }
        } catch { /* Original safe error and explicit uncertainty remain visible. */ }
      }
      return false;
    } finally { if (kind === 'starting') attempted.current.delete(key); if (pendingRef.current.get(id) === kind) mark(id, null); }
  };
  const start = async (chat: ChatSnapshot, request: StartTerminalTask): Promise<boolean> => {
    const current = live.current;
    if (!native || !current.ready || !current.config.ready || chat.active_run_id || !request.instruction.trim() || utf8Length(request.instruction) > MAX_MESSAGE_BYTES || !request.readiness_confirmed || request.chat_id !== chat.chat_id || request.context_consent.revision !== current.config.revision || request.context_consent.destination !== current.config.destination || current.session.state !== 'connected' || !sameSession(request.session, current.session.session)) return false;
    if (Object.values(tasksRef.current).some(task => task.owner || (!task.terminal_ready && task.phase === 'cancelled' && connected(task))) || pendingRef.current.size || uncertainRef.current.size) return false;
    const draft = draftsRef.current[chat.task_id];
    const key = `start:${chat.task_id}:${crypto.randomUUID()}`;
    const accepted = await mutate(chat.task_id, key, 'starting', () => agentBridge.start(request));
    if (accepted && alive.current && draftsRef.current[chat.task_id] === draft) updateDraft(chat.task_id, '');
    return accepted;
  };
  const matches = (identity: ActionIdentity) => {
    const task = tasksRef.current[identity.task_id];
    return task?.run_id === identity.run_id && task.action?.action_id === identity.action_id && task.action.display_revision === identity.display_revision ? task : null;
  };
  const connected = (task: TerminalTaskSnapshot) => live.current.session.state === 'connected' && sameSession(task.session, live.current.session.session);
  const acknowledge = async (identity: ActionIdentity) => {
    const task = matches(identity);
    if (!task || !task.owner || task.phase !== 'awaiting_display' || task.action?.displayed || !connected(task)) return;
    await mutate(task.task_id, `ack:${identity.run_id}:${identity.action_id}:${identity.display_revision}`, 'acknowledging', () => agentBridge.ack(identity));
  };
  const decide = async (identity: ActionIdentity, approvalId: string, decision: 'approve' | 'deny') => {
    const task = matches(identity);
    if (!task || !task.owner || task.phase !== 'waiting_approval' || !task.action?.displayed || task.action.approval_id !== approvalId || !connected(task)) return;
    await mutate(task.task_id, `decide:${identity.run_id}:${identity.action_id}:${approvalId}`, 'deciding', () => agentBridge.decide({ task_id: task.task_id, run_id: task.run_id, action_id: identity.action_id, approval_id: approvalId, decision }));
  };
  const cancel = async (taskId: string, runId: string) => {
    const task = tasksRef.current[taskId];
    if (!task || task.run_id !== runId || !task.owner) return;
    await mutate(taskId, `cancel:${runId}`, 'cancelling', () => agentBridge.cancel({ task_id: taskId, run_id: runId }));
  };
  const continueTask = async (taskId: string, runId: string) => {
    const task = tasksRef.current[taskId];
    if (!task || task.run_id !== runId || !connected(task) || (task.phase !== 'needs_user' && !(task.phase === 'cancelled' && !task.terminal_ready))) return;
    await mutate(taskId, `continue:${runId}:${task.revision}`, 'continuing', () => agentBridge.continue({ task_id: taskId, run_id: runId, readiness_confirmed: true }));
  };
  const owner = Object.values(tasks).find(task => task.owner) ?? null;
  const unresolved = Object.values(tasks).some(task => task.phase === 'cancelled' && !task.terminal_ready && connected(task));
  const blocked = !!owner || unresolved || Object.keys(pending).length > 0 || Object.keys(uncertain).length > 0;
  const retryLoad = () => { if (native && !ready) setLoadAttempt(value => value + 1); };
  return { native, ready, tasks, owner, blocked, error, errors, pending, uncertain, drafts, updateDraft, start, acknowledge, decide, cancel, continueTask, refresh, retryLoad, actionIdentity };
}
export type TerminalAgentController = ReturnType<typeof useTerminalAgent>;
