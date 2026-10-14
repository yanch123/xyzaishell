import { useCallback, useEffect, useRef, useState } from 'react';
import { desktopAvailable } from '../lib/bridge';
import { modelBridge } from '../lib/modelBridge';
import { MAX_CHATS, MAX_DRAFT_LENGTH, MAX_MESSAGE_BYTES, MAX_MESSAGES, utf8Length } from '../lib/chats';
import { INITIAL_MODEL_CONFIG, MAX_CONCURRENT_CHATS, newerRevision } from '../lib/model';
import type { ChatSnapshot, ModelConfigStatus } from '../lib/model';
import { safeError } from '../lib/types';

function previewChat(): ChatSnapshot {
  const id = crypto.randomUUID();
  return { chat_id: id, task_id: id, title: '新任务', messages: [], active_run_id: null, revision: '0' };
}

export function useModelChat() {
  const native = desktopAvailable();
  const [chats, setChats] = useState<ChatSnapshot[]>(() => native ? [] : [previewChat()]);
  const [config, setConfig] = useState<ModelConfigStatus>(INITIAL_MODEL_CONFIG);
  const [ready, setReady] = useState(!native);
  const [selected, setSelected] = useState<string | null>(null);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [errors, setErrors] = useState<Record<string, string | null>>({});
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<Record<string, 'sending' | 'cancelling'>>({});
  const [creating, setCreating] = useState(false);
  const [configBusy, setConfigBusy] = useState(false);
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
    const old = chatsRef.current.find(chat => chat.chat_id === next.chat_id);
    if (old && !newerRevision(next.revision, old.revision)) return;
    const updated = old ? chatsRef.current.map(chat => chat.chat_id === next.chat_id ? next : chat) : [...chatsRef.current, next];
    chatsRef.current = updated;
    setChats(updated);
  }, []);
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
      const active = chatsRef.current.filter(chat => chat.active_run_id || pendingRef.current.has(chat.chat_id));
      if (!active.length) return;
      reading = true;
      await Promise.all(active.map(async chat => {
        try { const next = await modelBridge.get(chat.chat_id); if (!disposed) acceptChat(next); }
        catch (failure) { if (!disposed) setErrors(items => ({ ...items, [chat.chat_id]: safeError(failure) })); }
      }));
      reading = false;
    };
    // Full snapshots repair dropped final events; these reads never restart runs.
    const timer = window.setInterval(() => void reconcile(), 4000);
    const onVisible = () => { if (document.visibilityState === 'visible') void reconcile(); };
    document.addEventListener('visibilitychange', onVisible);
    return () => { disposed = true; window.clearInterval(timer); document.removeEventListener('visibilitychange', onVisible); };
  }, [native, ready, acceptChat]);

  const retryLoad = () => { if (native && !ready) { setError(null); initialChat.current = null; setLoadAttempt(value => value + 1); } };

  const current = chats.find(chat => chat.chat_id === selected) ?? chats[0] ?? null;
  const updateDraft = (chatId: string, value: string) => {
    const next = { ...draftsRef.current, [chatId]: value.slice(0, MAX_DRAFT_LENGTH) };
    draftsRef.current = next;
    setDrafts(next);
  };
  const createChat = async () => {
    if (!ready || creatingRef.current || chatsRef.current.length >= MAX_CHATS) return;
    creatingRef.current = true;
    setCreating(true);
    setError(null);
    try {
      const created = native ? await modelBridge.create() : previewChat();
      if (alive.current) { acceptChat(created); setSelected(created.chat_id); }
    } catch (failure) { if (alive.current) setError(safeError(failure)); }
    finally { creatingRef.current = false; if (alive.current) setCreating(false); }
  };
  const selectChat = (chatId: string) => {
    setSelected(chatId);
    // Read-only reconciliation: never cancels, resends or transfers a run.
    if (native && ready) void modelBridge.get(chatId).then(next => {
      if (alive.current) acceptChat(next);
    }).catch(failure => { if (alive.current) setErrors(items => ({ ...items, [chatId]: safeError(failure) })); });
  };
  const send = async (chatId: string): Promise<boolean> => {
    const chat = chatsRef.current.find(item => item.chat_id === chatId);
    const text = draftsRef.current[chatId]?.trim() ?? '';
    if (!native || !ready || !chat || !text || utf8Length(text) > MAX_MESSAGE_BYTES || chat.messages.length > MAX_MESSAGES - 2 || chat.active_run_id || pendingRef.current.has(chatId) || !configRef.current.ready || !configRef.current.consented) return false;
    if (chatsRef.current.filter(item => item.active_run_id || pendingRef.current.get(item.chat_id) === 'sending').length >= MAX_CONCURRENT_CHATS) return false;
    markPending(chatId, 'sending');
    setErrors(items => ({ ...items, [chatId]: null }));
    const submittedDraft = draftsRef.current[chatId];
    try {
      const next = await modelBridge.send(chatId, text);
      if (alive.current) {
        acceptChat(next);
        // A user may already have begun their next message while IPC completes.
        if (draftsRef.current[chatId] === submittedDraft) updateDraft(chatId, '');
      }
      return true;
    } catch (failure) {
      if (alive.current) {
        setErrors(items => ({ ...items, [chatId]: safeError(failure) }));
        // An IPC failure can be ambiguous; reconcile without reissuing the send.
        try { const next = await modelBridge.get(chatId); if (alive.current) acceptChat(next); } catch { /* Keep safe original error. */ }
      }
      return false;
    } finally { if (alive.current) markPending(chatId, null); else pendingRef.current.delete(chatId); }
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
    try { const next = await modelBridge.reload(); if (alive.current) acceptConfig(next); }
    catch (failure) { if (alive.current) setError(safeError(failure)); }
    finally { configBusyRef.current = false; if (alive.current) setConfigBusy(false); }
  };
  const confirmDestination = async (expected: ModelConfigStatus): Promise<boolean> => {
    if (!native || configBusyRef.current || !expected.ready || !expected.revision || !expected.destination) return false;
    configBusyRef.current = true; setConfigBusy(true); setError(null);
    try {
      const next = await modelBridge.confirm(expected.revision, expected.destination);
      if (alive.current) acceptConfig(next);
      return next.consented;
    } catch (failure) { if (alive.current) setError(safeError(failure)); return false; }
    finally { configBusyRef.current = false; if (alive.current) setConfigBusy(false); }
  };
  const probe = async (): Promise<boolean> => {
    if (!native || !ready || !configRef.current.consented || !configRef.current.ready || creatingRef.current || chatsRef.current.length >= MAX_CHATS) return false;
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
  return { native, ready, config, chats, current, drafts, errors, error, pending, creating, configBusy, runningCount, updateDraft, createChat, selectChat, send, cancel, reloadConfig, confirmDestination, probe, retryLoad, restoreChat };
}
export type ModelChatController = ReturnType<typeof useModelChat>;
