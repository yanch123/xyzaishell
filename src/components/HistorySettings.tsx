import { useEffect, useRef, useState } from 'react';
import { historyBridge } from '../lib/storageBridge';
import type { HistoryEntry, HistoryStatus } from '../lib/storageBridge';
import type { ChatSnapshot } from '../lib/model';
import { safeError } from '../lib/types';
const manageable = (status: HistoryStatus | null) => !!(status?.enabled || status?.manageable);
export function HistorySettings({ native, onRestore, onStatus }: { native: boolean; onRestore: (chat: ChatSnapshot) => void; onStatus?: (status: HistoryStatus) => void }) {
  const [status, setStatus] = useState<HistoryStatus | null>(null);
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [consent, setConsent] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [forgetting, setForgetting] = useState<HistoryEntry | null>(null);
  const [forgetUncertain, setForgetUncertain] = useState(false);
  const lock = useRef(false);
  const lifecycle = useRef(0);
  const mounted = useRef(false);
  const listRequest = useRef(0);
  const refreshEntries = async (next: HistoryStatus) => {
    const epoch = lifecycle.current; const request = ++listRequest.current;
    const saved = manageable(next) ? await historyBridge.list() : [];
    if (mounted.current && lifecycle.current === epoch && request === listRequest.current) setEntries(saved);
  };
  useEffect(() => {
    mounted.current = true;
    const epoch = lifecycle.current;
    let stop: (() => void) | undefined;
    const current = () => mounted.current && lifecycle.current === epoch;
    const accept = (next: HistoryStatus) => { if (current()) { setStatus(next); void refreshEntries(next).catch(e => { if (current()) setError(safeError(e)); }); } };
    if (native) {
      void historyBridge.status().then(accept).catch(e => { if (current()) setError(safeError(e)); });
      void historyBridge.listen(accept).then(unlisten => { if (current()) stop = unlisten; else unlisten(); }).catch(e => { if (current()) setError(safeError(e)); });
    }
    return () => { mounted.current = false; lifecycle.current += 1; listRequest.current += 1; stop?.(); };
  }, [native]);
  const run = async (operation: (current: () => boolean) => Promise<void>) => {
    if (lock.current || !mounted.current) return;
    const epoch = lifecycle.current;
    const current = () => mounted.current && lifecycle.current === epoch;
    lock.current = true; setBusy(true); setError(null);
    try { await operation(current); }
    catch (e) { if (current()) setError(safeError(e)); }
    finally { lock.current = false; if (current()) setBusy(false); }
  };
  const enable = (enabled: boolean) => {
    if (enabled && !consent) return;
    return run(async current => { const next = await historyBridge.enable(enabled); if (!current()) return; setStatus(next); onStatus?.(next); setConsent(false); await refreshEntries(next); });
  };
  const restore = (entry: HistoryEntry) => {
    if (!manageable(status)) return;
    return run(async current => { const chat = await historyBridge.restore(entry.chat_id, entry.revision); if (!current()) return; if (chat.active_run_id) throw new Error('Active history rejected'); onRestore(chat); });
  };
  const forget = () => {
    const entry = forgetting;
    if (!entry || forgetUncertain || !manageable(status)) return;
    return run(async current => {
      try {
        const next = await historyBridge.forget(entry.chat_id, entry.revision);
        if (!current()) return;
        setStatus(next); onStatus?.(next); setForgetting(null);
        setEntries(items => items.filter(item => item.chat_id !== entry.chat_id));
        await refreshEntries(next);
      } catch (e) { if (current()) setForgetUncertain(true); throw e; }
    });
  };
  return <div className="settings-section"><div className="settings-content"><h3>本机聊天历史 <span className="subtle-badge">明文 · 默认关闭</span></h3>
    <p>启用后将当前及后续聊天文本保存在本机明文文件中。已知凭据会脱敏，但不能保证识别所有秘密；请勿把密码、私钥或敏感资料写入聊天。不会保存原始终端内容或可重放的执行授权。</p>
    <p>每次启动需重新选择启用。关闭只停止新增保存，不删除已有文件。恢复只读取文字，不恢复连接、启动模型、批准动作或重发命令。</p>
    {!native && <p className="field-hint">网页预览无法持久保存或恢复聊天。</p>}
    {status?.error && <p className="inline-error" role="alert">{status.error.safe_message}</p>}
    {error && <p className="inline-error" role="alert">{error}</p>}
    {!status?.enabled ? <><label className="check-field"><input type="checkbox" checked={consent} disabled={!native || !status?.available || busy} onChange={e => setConsent(e.target.checked)} /><span>我理解聊天将以明文保存到本机，并同意本次进程启用。</span></label><button className="button secondary" disabled={!consent || !status?.available || busy} onClick={() => void enable(true)}>启用明文聊天历史</button></> : <button className="button secondary" disabled={busy} onClick={() => void enable(false)}>停止保存聊天历史</button>}
    {manageable(status) && <>
      {!status?.enabled && <p className="field-hint">已停止新增保存，仍可管理本次已打开的历史。</p>}
      <button className="button secondary" disabled={busy} onClick={() => void run(async current => { const next = await historyBridge.status(); if (!current()) return; setStatus(next); await refreshEntries(next); if (current()) { setForgetUncertain(false); setForgetting(null); } })}>重新读取历史列表</button>
      <div className="saved-history-list" aria-label="本机保存的聊天">{entries.length ? entries.map(entry => <div key={entry.chat_id}><span>{entry.title} · {entry.message_count} 条消息</span><button className="button secondary" disabled={busy} onClick={() => void restore(entry)}>仅恢复文字</button><button className="button secondary" aria-label={`移除历史：${entry.title}`} disabled={busy || forgetUncertain} onClick={() => setForgetting(entry)}>移除历史</button></div>) : <p className="field-hint">尚无可恢复的聊天。</p>}</div>
      {forgetting && <div className="notice warning" role="group" aria-label="确认移除单条历史"><p>确定移除“{forgetting.title}”的本机历史记录（记录 ID：{forgetting.chat_id}）？此操作不可撤销，也不保证底层磁盘、备份或其他副本中的数据被物理擦除。当前打开的聊天仍在内存中，本次进程不会自动重新保存这条记录。</p><div className="settings-actions"><button className="button secondary" disabled={busy} onClick={() => setForgetting(null)}>取消移除</button><button className="button danger-button" disabled={busy || forgetUncertain} onClick={() => void forget()}>确认移除此条历史</button></div></div>}
      {forgetUncertain && <p className="notice warning" role="alert">移除结果可能不确定，已阻止重复提交。请重新读取历史列表核对。</p>}
    </>}
  </div></div>;
}
