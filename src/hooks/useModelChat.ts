import type { TerminalTaskSnapshot } from '../lib/agent';
import { useCallback, useEffect, useRef, useState } from 'react';
import { desktopAvailable } from '../lib/bridge';
import { modelBridge } from '../lib/modelBridge';
import { MAX_CHATS, MAX_DRAFT_LENGTH, MAX_MESSAGE_BYTES, MAX_MESSAGES, utf8Length } from '../lib/chats';
import { INITIAL_MODEL_CONFIG, MAX_CONCURRENT_CHATS, newerRevision } from '../lib/model';
import type { ChatSnapshot, ModelConfigStatus, SaveModelConfigRequest, ModelTestResult, ModelTestKind } from '../lib/model';
import type { SessionSnapshot } from '../lib/types';
import { safeError, sameSession } from '../lib/types';
import type { TerminalAttachmentDraft, TerminalSelection } from '../lib/terminalSelection';
import { attachmentConsented, boundSelection, composeAttachedMessage, selectionMatchesChat, selectionMatchesTarget } from '../lib/terminalSelection';

// These errors are returned before a unified turn is accepted. Unknown failures
// may follow real execution and must never unlock a replay of the same draft.
const SEND_PREFLIGHT_ERRORS = new Set(['MODEL_CONFIG_CHANGED', 'MODEL_CONFIG_UNAVAILABLE', 'CONVERSATION_SCOPE_REQUIRED', 'SESSION_STALE', 'CHAT_BUSY', 'CHAT_NOT_FOUND', 'AI_OWNER_BUSY', 'CHAT_RESTORED_READ_ONLY', 'CHAT_TARGET_MISMATCH', 'TASK_INPUT_INVALID']);
type UncertainSubmission = { submissionId?: string; revision: string; text: string; draft: string | undefined; messageIds: string[]; attachment?: TerminalAttachmentDraft };

function previewChat(): ChatSnapshot {
  const id = crypto.randomUUID();
  return { chat_id: id, task_id: id, title: '新任务', messages: [], active_run_id: null, revision: '0' };
}

function targetIdentity(target?: SessionSnapshot): string | null {
  return target?.state === 'connected' && target.session && target.host && target.port && target.username
    ? JSON.stringify([target.host, target.port, target.username]) : null;
}
function chatMatchesTarget(chat: ChatSnapshot, target?: SessionSnapshot): boolean {
  if (chat.restored_read_only) return false;
  return !chat.source || !targetIdentity(target) || chat.source.host === target?.host && chat.source.port === target?.port && chat.source.username === target?.username;
}
export function useModelChat(target?: SessionSnapshot) {
  const native = desktopAvailable();
  const [terminalTask, setTerminalTask] = useState<TerminalTaskSnapshot | null>(null);
  const [scopeGrant, setScopeGrant] = useState<string | null>(null);
  const scopeGrantRef = useRef<string | null>(null);
  const scopeKey = (configuration: ModelConfigStatus, session?: SessionSnapshot) => JSON.stringify([configuration.revision, configuration.destination, targetIdentity(session)]);
  const [chats, setChats] = useState<ChatSnapshot[]>(() => native ? [] : [previewChat()]);
  const [config, setConfig] = useState<ModelConfigStatus>(INITIAL_MODEL_CONFIG);
  const [ready, setReady] = useState(!native);
  const [selected, setSelected] = useState<string | null>(null);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [attachments, setAttachments] = useState<Record<string, TerminalAttachmentDraft>>({});
  const attachmentsRef = useRef(attachments);
  const [errors, setErrors] = useState<Record<string, string | null>>({});
  const [error, setError] = useState<string | null>(null);
  const [uncertainSubmissions, setUncertainSubmissions] = useState<Record<string, boolean>>({});
  const uncertainSubmissionsRef = useRef(new Map<string, UncertainSubmission>());
  const [pending, setPending] = useState<Record<string, 'sending' | 'cancelling'>>({});
  const [creating, setCreating] = useState(false);
  const [configBusy, setConfigBusy] = useState(false);
  const [configWriteBlocked, setConfigWriteBlocked] = useState(false);
  const [configTest, setConfigTest] = useState<ModelTestResult | null>(null);
  const [configTestBusy, setConfigTestBusy] = useState(false);
  const testIdRef = useRef<string | null>(null);
  const targetRef = useRef(target);
  targetRef.current = target;
  const lastTargetRef = useRef<string | null>(null);
  const [loadAttempt, setLoadAttempt] = useState(0);
  const chatsRef = useRef(chats);
  const configRef = useRef(config);
  const draftsRef = useRef(drafts);
  const pendingRef = useRef(new Map<string, 'sending' | 'cancelling'>());
  const creatingRef = useRef(false);
  const configBusyRef = useRef(false);
  const alive = useRef(false);
  const initialChat = useRef<Promise<ChatSnapshot> | null>(null);

  const acceptChat = useCallback((next: ChatSnapshot) => {
    const uncertain = uncertainSubmissionsRef.current.get(next.chat_id);
    if (uncertain && !uncertain.submissionId && newerRevision(next.revision, uncertain.revision) && next.messages.some(message => message.role === 'user' && !uncertain.messageIds.includes(message.id) && message.text === uncertain.text)) {
      // A newer authoritative transcript proves acceptance, even if execution
      // already finished. Consume the original draft before lifting the fence.
      if (draftsRef.current[next.chat_id] === uncertain.draft) {
        draftsRef.current = { ...draftsRef.current, [next.chat_id]: '' }; setDrafts(draftsRef.current);
      }
      if (uncertain.attachment && attachmentsRef.current[next.chat_id] === uncertain.attachment) {
        const remaining = { ...attachmentsRef.current }; delete remaining[next.chat_id];
        attachmentsRef.current = remaining; setAttachments(remaining);
      }
      uncertainSubmissionsRef.current.delete(next.chat_id);
      setUncertainSubmissions(Object.fromEntries([...uncertainSubmissionsRef.current.keys()].map(id => [id, true])));
    }
    const old = chatsRef.current.find(chat => chat.chat_id === next.chat_id);
    if (old && !newerRevision(next.revision, old.revision)) return;
    const updated = old ? chatsRef.current.map(chat => chat.chat_id === next.chat_id ? next : chat) : [...chatsRef.current, next];
    chatsRef.current = updated;
    setChats(updated);
  }, []);
  const reconcileById = useCallback(async (chatId: string) => {
    const uncertain = uncertainSubmissionsRef.current.get(chatId);
    if (!uncertain?.submissionId) return;
    const result = await modelBridge.submission(uncertain.submissionId);
    if (!alive.current || !result || uncertainSubmissionsRef.current.get(chatId) !== uncertain) return;
    const returnedChatId = result.kind === 'chat' ? result.chat.chat_id : result.task.chat_id;
    if (returnedChatId !== chatId) return;
    // The host's submission lookup is authoritative even after redaction or a
    // completed run. It never resends or obtains new execution authority.
    if (result.kind === 'terminal') setTerminalTask(result.task); else acceptChat(result.chat);
    if (draftsRef.current[chatId] === uncertain.draft) {
      draftsRef.current = { ...draftsRef.current, [chatId]: '' }; setDrafts(draftsRef.current);
    }
    uncertainSubmissionsRef.current.delete(chatId);
    setUncertainSubmissions(Object.fromEntries([...uncertainSubmissionsRef.current.keys()].map(id => [id, true])));
  }, [acceptChat]);
  const acceptConfig = useCallback((next: ModelConfigStatus) => {
    const old = configRef.current;
    if (old.revision && (!next.revision || !newerRevision(next.revision, old.revision))) return;
    configRef.current = next;
    setConfig(next);
  }, []);
  const markPending = (id: string, value: 'sending' | 'cancelling' | null) => {
    if (value) pendingRef.current.set(id, value); else pendingRef.current.delete(id);
    setPending(Object.fromEntries(pendingRef.current));
  };

  useEffect(() => {
    alive.current = true;
    if (!native) return () => { alive.current = false; };
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await modelBridge.listen(event => {
          if (disposed) return;
          if (event.kind === 'chat_updated') acceptChat(event.chat);
          else if (event.kind === 'config_updated') acceptConfig(event.config);
        });
        if (disposed) { unlisten(); return; }
        const [configuration, saved] = await Promise.all([modelBridge.config(), modelBridge.list()]);
        if (disposed) return;
        acceptConfig(configuration);
        saved.forEach(acceptChat);
        if (!saved.length && !chatsRef.current.length) {
          initialChat.current ??= modelBridge.create();
          const created = await initialChat.current;
          if (disposed) return;
          acceptChat(created);
        }
        setReady(true);
      } catch (failure) { if (!disposed) setError(safeError(failure)); }
    })();
    return () => { alive.current = false; disposed = true; unlisten?.(); };
  }, [native, acceptChat, acceptConfig, loadAttempt]);

  useEffect(() => {
    if (!native || !ready) return;
    let disposed = false;
    let reading = false;
    const reconcile = async () => {
      if (reading || disposed) return;
      const active = chatsRef.current.filter(chat => chat.active_run_id || pendingRef.current.has(chat.chat_id) || uncertainSubmissionsRef.current.has(chat.chat_id));
      if (!active.length) return;
      reading = true;
      await Promise.all(active.map(async chat => {
        try { await reconcileById(chat.chat_id); const next = await modelBridge.get(chat.chat_id); if (!disposed) acceptChat(next); }
        catch (failure) { if (!disposed) setErrors(items => ({ ...items, [chat.chat_id]: safeError(failure) })); }
      }));
      reading = false;
    };
    // Full snapshots repair dropped final events; these reads never restart runs.
    const timer = window.setInterval(() => void reconcile(), 4000);
    const onVisible = () => { if (document.visibilityState === 'visible') void reconcile(); };
    document.addEventListener('visibilitychange', onVisible);
    return () => { disposed = true; window.clearInterval(timer); document.removeEventListener('visibilitychange', onVisible); };
  }, [native, ready, acceptChat, reconcileById]);

  const retryLoad = () => { if (native && !ready) { setError(null); initialChat.current = null; setLoadAttempt(value => value + 1); } };

  const current = chats.find(chat => chat.chat_id === selected) ?? chats[0] ?? null;
  const updateDraft = (chatId: string, value: string) => {
    const next = { ...draftsRef.current, [chatId]: value.slice(0, MAX_DRAFT_LENGTH) };
    draftsRef.current = next;
    setDrafts(next);
  };
  const commitAttachments = (next: Record<string, TerminalAttachmentDraft>) => { attachmentsRef.current = next; setAttachments(next); };
  const revokeAttachmentConsents = () => {
    if (!Object.values(attachmentsRef.current).some(item => item.consent)) return;
    commitAttachments(Object.fromEntries(Object.entries(attachmentsRef.current).map(([id, item]) => [id, Object.freeze({ ...item, consent: null })])));
  };
  const attachmentContext = JSON.stringify([current?.chat_id, target?.state, target?.session?.session_id, target?.session?.generation, target?.host, target?.port, target?.username, config.revision, config.destination]);
  useEffect(() => { revokeAttachmentConsents(); }, [attachmentContext]);
  const attachSelection = (chatId: string, selection: TerminalSelection): boolean => {
    const chat = chatsRef.current.find(item => item.chat_id === chatId);
    if (!chat || !selectionMatchesTarget(selection, targetRef.current) || !selectionMatchesChat(selection, chat) || attachmentsRef.current[chatId]) return false;
    const bounded = boundSelection(selection.text);
    if (!bounded.text.trim()) return false;
    const attachment: TerminalAttachmentDraft = Object.freeze({ ...selection, source: Object.freeze({ ...selection.source }), ...bounded, truncated: selection.truncated || bounded.truncated, edited: false, consent: null });
    commitAttachments({ ...attachmentsRef.current, [chatId]: attachment });
    return true;
  };
  const editAttachment = (chatId: string, text: string) => {
    const attachment = attachmentsRef.current[chatId];
    if (!attachment || attachment.text === text) return;
    const bounded = boundSelection(text);
    commitAttachments({ ...attachmentsRef.current, [chatId]: Object.freeze({ ...attachment, text: bounded.text, truncated: attachment.truncated || bounded.truncated, edited: true, consent: null }) });
  };
  const removeAttachment = (chatId: string) => { const next = { ...attachmentsRef.current }; delete next[chatId]; commitAttachments(next); };
  const consentAttachment = (chatId: string, allowed: boolean) => {
    const attachment = attachmentsRef.current[chatId]; const config = configRef.current;
    const chat = chatsRef.current.find(item => item.chat_id === chatId);
    if (!attachment) return;
    const consent = allowed && config.ready && config.revision && config.destination && chat && selectionMatchesTarget(attachment, targetRef.current) && selectionMatchesChat(attachment, chat)
      ? Object.freeze({ revision: config.revision, destination: config.destination }) : null;
    commitAttachments({ ...attachmentsRef.current, [chatId]: Object.freeze({ ...attachment, consent }) });
  };
  const createChat = async () => {
    if (!ready || creatingRef.current || chatsRef.current.length >= MAX_CHATS) return;
    creatingRef.current = true;
    setCreating(true);
    setError(null);
    try {
      const expectedTarget = targetIdentity(targetRef.current);
      const created = native ? await modelBridge.create('新任务', expectedTarget ? targetRef.current!.session : null) : previewChat();
      if (alive.current) { acceptChat(created); if (expectedTarget === targetIdentity(targetRef.current)) setSelected(created.chat_id); }
    } catch (failure) { if (alive.current) setError(safeError(failure)); }
    finally { creatingRef.current = false; if (alive.current) setCreating(false); }
  };
  const connectedTarget = targetIdentity(target);
  useEffect(() => {
    if (!native || !ready || !connectedTarget || lastTargetRef.current === connectedTarget || creatingRef.current || chatsRef.current.length >= MAX_CHATS) return;
    if (lastTargetRef.current === null) {
      const existing = chatsRef.current.find(chat => chat.source && chatMatchesTarget(chat, targetRef.current));
      if (existing) { lastTargetRef.current = connectedTarget; setSelected(existing.chat_id); return; }
    }
    lastTargetRef.current = connectedTarget;
    creatingRef.current = true; setCreating(true);
    const source = targetRef.current?.session ?? null;
    void modelBridge.create('新任务', source).then(created => {
      if (alive.current) { acceptChat(created); if (targetIdentity(targetRef.current) === connectedTarget) setSelected(created.chat_id); }
    }).catch(failure => {
      if (alive.current) setError(safeError(failure));
    }).finally(() => { creatingRef.current = false; if (alive.current) setCreating(false); });
  }, [native, ready, connectedTarget, creating, acceptChat]);
  const selectChat = (chatId: string) => {
    if (chatId !== current?.chat_id) revokeAttachmentConsents();
    setSelected(chatId);
    // Read-only reconciliation: never cancels, resends or transfers a run.
    if (native && ready) void modelBridge.get(chatId).then(next => {
      if (alive.current) acceptChat(next);
    }).catch(failure => { if (alive.current) setErrors(items => ({ ...items, [chatId]: safeError(failure) })); });
  };
  const send = async (chatId: string): Promise<boolean> => {
    const chat = chatsRef.current.find(item => item.chat_id === chatId);
    const attachment = attachmentsRef.current[chatId];
    if (attachment && (!chat || !selectionMatchesTarget(attachment, targetRef.current) || !selectionMatchesChat(attachment, chat) || !attachmentConsented(attachment, configRef.current) || !attachment.text.trim())) {
      setErrors(items => ({ ...items, [chatId]: '终端选区的来源、会话或接收方已变化，或尚未审阅确认。请移除旧附件后从当前终端重新选择并核对。' }));
      return false;
    }
    const text = composeAttachedMessage(draftsRef.current[chatId] ?? '', attachment);
    if (!native || !ready || configWriteBlocked || !chat || !chatMatchesTarget(chat, targetRef.current) || !text || utf8Length(text) > MAX_MESSAGE_BYTES || chat.messages.length > MAX_MESSAGES - 2 || chat.active_run_id || pendingRef.current.has(chatId) || uncertainSubmissionsRef.current.has(chatId) || !configRef.current.ready || !configRef.current.consented || (!attachment && targetIdentity(targetRef.current) && scopeGrantRef.current !== scopeKey(configRef.current, targetRef.current))) return false;
    if (chatsRef.current.filter(item => item.active_run_id || pendingRef.current.get(item.chat_id) === 'sending').length >= MAX_CONCURRENT_CHATS) return false;
    markPending(chatId, 'sending');
    setErrors(items => ({ ...items, [chatId]: null }));
    const submittedDraft = draftsRef.current[chatId];
    const submissionId = attachment ? undefined : crypto.randomUUID();
    try {
      const next = attachment
        ? { kind: 'chat' as const, chat: await modelBridge.sendText(chatId, text) }
        : await modelBridge.send(chatId, text, configRef.current.revision!, configRef.current.destination!, targetRef.current?.state === 'connected' ? targetRef.current.session : null, submissionId!);
      if (alive.current) {
        if (next.kind === 'terminal') setTerminalTask(next.task);
        else if (next.kind === 'chat') acceptChat(next.chat);
        // A user may already have begun their next message while IPC completes.
        if (draftsRef.current[chatId] === submittedDraft) updateDraft(chatId, '');
        if (attachment && attachmentsRef.current[chatId] === attachment) removeAttachment(chatId);
      }
      return true;
    } catch (failure) {
      if (alive.current) {
        setErrors(items => ({ ...items, [chatId]: safeError(failure) }));
        const code = typeof failure === 'object' && failure !== null && 'code' in failure ? String(failure.code) : null;
        if (!code || !SEND_PREFLIGHT_ERRORS.has(code)) {
          uncertainSubmissionsRef.current.set(chatId, { submissionId, revision: chat.revision, text, draft: submittedDraft, messageIds: chat.messages.map(message => message.id), attachment });
          setUncertainSubmissions(items => ({ ...items, [chatId]: true }));
          // An event may have arrived before the failed IPC response.
          const known = chatsRef.current.find(item => item.chat_id === chatId);
          if (known) acceptChat(known);
        }
        // Only authoritative reads can reconcile this fence; never resubmit.
        try { await reconcileById(chatId); const next = await modelBridge.get(chatId); if (alive.current) acceptChat(next); } catch { /* Keep safe original error and submission fence. */ }
      }
      return false;
    } finally { if (alive.current) markPending(chatId, null); else pendingRef.current.delete(chatId); }
  };
  const reconcileSubmission = async (chatId: string) => {
    if (!native || !uncertainSubmissionsRef.current.has(chatId)) return;
    try { await reconcileById(chatId); const next = await modelBridge.get(chatId); if (alive.current) acceptChat(next); }
    catch (failure) { if (alive.current) setErrors(items => ({ ...items, [chatId]: safeError(failure) })); }
  };
  const cancel = async (chatId: string) => {
    const runId = chatsRef.current.find(chat => chat.chat_id === chatId)?.active_run_id;
    if (!native || !runId || pendingRef.current.has(chatId)) return;
    markPending(chatId, 'cancelling');
    setErrors(items => ({ ...items, [chatId]: null }));
    try { const next = await modelBridge.cancel(chatId, runId); if (alive.current) acceptChat(next); }
    catch (failure) { if (alive.current) setErrors(items => ({ ...items, [chatId]: safeError(failure) })); }
    finally { if (alive.current) markPending(chatId, null); else pendingRef.current.delete(chatId); }
  };
  const reloadConfig = async () => {
    if (!native || configBusyRef.current) return;
    configBusyRef.current = true; setConfigBusy(true); setError(null);
    try { const next = await modelBridge.reload(); if (alive.current) { acceptConfig(next); setConfigWriteBlocked(false); } }
    catch (failure) { if (alive.current) setError(safeError(failure)); }
    finally { configBusyRef.current = false; if (alive.current) setConfigBusy(false); }
  };
  const saveConfig = async (request: SaveModelConfigRequest): Promise<ModelConfigStatus | null> => {
    if (!native || configBusyRef.current || configWriteBlocked) return null;
    configBusyRef.current = true; setConfigBusy(true); setError(null);
    try { const next = await modelBridge.save(request); if (alive.current) acceptConfig(next); return next; }
    catch (failure) {
      if (alive.current) {
        setError(safeError(failure));
        const code = typeof failure === 'object' && failure !== null && 'code' in failure ? String(failure.code) : null;
        if (!code || code === 'CONFIG_CONFLICT' || code === 'CONFIG_SAVE_UNCERTAIN') setConfigWriteBlocked(true);
      }
      return null;
    }
    finally { configBusyRef.current = false; if (alive.current) setConfigBusy(false); }
  };
  const testSaved = async (expected: ModelConfigStatus, kind: ModelTestKind = 'chat_connectivity'): Promise<ModelTestResult | null> => {
    if (!native || configWriteBlocked || testIdRef.current || !expected.ready || !expected.file_revision || !expected.destination || !expected.model) return null;
    const testId = crypto.randomUUID(); testIdRef.current = testId; setConfigTestBusy(true); setError(null); setConfigTest(null);
    try {
      const result = await modelBridge.testSaved({ test_id: testId, file_revision: expected.file_revision, destination: expected.destination, model: expected.model, consent: true, kind });
      if (alive.current) setConfigTest(result);
      return result;
    } catch (failure) { if (alive.current) setError(safeError(failure)); return null; }
    finally { testIdRef.current = null; if (alive.current) setConfigTestBusy(false); }
  };
  const cancelConfigTest = async () => {
    if (!testIdRef.current) return;
    try { await modelBridge.cancelTest(testIdRef.current); }
    catch (failure) { if (alive.current) setError(safeError(failure)); }
  };
  const confirmDestination = async (expected: ModelConfigStatus, expectedTarget: SessionSnapshot | undefined = targetRef.current): Promise<boolean> => {
    if (!native || configWriteBlocked || configBusyRef.current || !expected.ready || !expected.revision || !expected.destination) return false;
    configBusyRef.current = true; setConfigBusy(true); setError(null);
    try {
      if (scopeKey(expected, expectedTarget) !== scopeKey(configRef.current, targetRef.current) || expectedTarget?.state !== targetRef.current?.state || expectedTarget?.state === 'connected' && !sameSession(expectedTarget.session, targetRef.current?.session)) return false;
      const next = await modelBridge.confirmScope(expected.revision, expected.destination, expectedTarget?.state === 'connected' ? expectedTarget.session : null);
      if (alive.current) {
        acceptConfig(next);
        const grant = scopeKey(next, expectedTarget); scopeGrantRef.current = grant; setScopeGrant(grant);
      }
      return next.consented;
    } catch (failure) { if (alive.current) setError(safeError(failure)); return false; }
    finally { configBusyRef.current = false; if (alive.current) setConfigBusy(false); }
  };
  const probe = async (): Promise<boolean> => {
    if (!native || !ready || configWriteBlocked || !configRef.current.consented || !configRef.current.ready || creatingRef.current || chatsRef.current.length >= MAX_CHATS) return false;
    creatingRef.current = true; setCreating(true); setError(null);
    try {
      const chat = await modelBridge.probe();
      if (alive.current) { acceptChat(chat); setSelected(chat.chat_id); }
      return true;
    } catch (failure) { if (alive.current) setError(safeError(failure)); return false; }
    finally { creatingRef.current = false; if (alive.current) setCreating(false); }
  };
  const restoreChat = (chat: ChatSnapshot) => {
    // Native restore only returns inactive text. Never issue send, start, or reconnect.
    if (chat.active_run_id) return;
    acceptChat(chat); setSelected(chat.chat_id);
  };
  const runningCount = chats.filter(chat => chat.active_run_id || pending[chat.chat_id] === 'sending').length;
  const scopeConsented = config.consented && (!connectedTarget || scopeGrant === scopeKey(config, target));
  return { uncertainSubmissions, reconcileSubmission, terminalTask, scopeConsented, native, ready, config, chats, current, drafts, attachments, attachSelection, editAttachment, removeAttachment, consentAttachment, errors, error, pending, creating, configBusy, runningCount, configWriteBlocked, configTest, configTestBusy, saveConfig, testSaved, cancelConfigTest, updateDraft, createChat, selectChat, send, cancel, reloadConfig, confirmDestination, probe, retryLoad, restoreChat };
}
export type ModelChatController = ReturnType<typeof useModelChat>;
