import { useEffect, useRef, useState } from 'react';
import { historyBridge, type HistoryPage } from '../lib/storageBridge';
import { safeError } from '../lib/types';
/** Display-only page cache. It is never passed to useModelChat or a send API. */
export function ArchivedConversation({ chatId }: { chatId: string }) {
  const [page, setPage] = useState<HistoryPage | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const revision = useRef<string | null>(null);
  const lock = useRef(false);
  const request = useRef(0);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const load = async (older: boolean) => {
    if (lock.current) return;
    const serial = ++request.current;
    lock.current = true; setBusy(true); setError(null);
    try {
      if (!older || !revision.current) {
        const entry = (await historyBridge.list()).find(item => item.chat_id === chatId);
        if (!entry) throw { code: 'HISTORY_NOT_SAVED', safe_message: '此聊天尚未保存，没有可读取的归档页。' };
        revision.current = entry.revision;
      }
      const next = await historyBridge.page(chatId, revision.current!, older ? page?.next_before ?? null : null);
      if (!alive.current || serial !== request.current) return;
      if (next.chat.chat_id !== chatId || !next.chat.restored_read_only || next.chat.active_run_id) throw { code: 'HISTORY_PAGE_INVALID', safe_message: '历史页状态无效，未载入。' };
      setPage(next); setOpen(true);
    } catch (failure) { if (alive.current && serial === request.current) setError(safeError(failure)); }
    finally { lock.current = false; if (alive.current) setBusy(false); }
  };
  return <section className="archived-conversation" aria-label="已保存对话分页">
    <button className="button secondary" disabled={busy} onClick={() => void load(false)}>{busy ? '正在读取历史…' : '翻阅已保存对话'}</button>
    {error && <p role="alert">{error}</p>}
    {open && page && <div><p>已保存 {page.total_messages} 条 · 当前页 {page.chat.messages.length} 条。翻页仅供阅读，不加入模型上下文或恢复执行权限。</p><div className="settings-actions"><button disabled={busy || page.next_before === null} onClick={() => void load(true)}>更早一页</button><button disabled={busy} onClick={() => void load(false)}>最新一页</button><button onClick={() => { ++request.current; setOpen(false); setPage(null); }}>收起历史页</button></div><div role="log" aria-label="归档历史页" aria-live="off">{page.chat.messages.map(message => <article className="local-message" key={message.id}><strong>{message.role === 'user' ? '你' : '模型'}</strong><p>{message.text}</p></article>)}</div></div>}
  </section>;
}
