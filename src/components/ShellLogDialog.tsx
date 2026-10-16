import { useEffect, useRef, useState } from 'react';
import { shellLogBridge } from '../lib/shellLogBridge';
import type { CaptureState, ExportFormat, ShellLogExport, ShellLogPage, ShellLogQuery, ShellLogStatus } from '../lib/shellLogBridge';
import { safeError, sameSession } from '../lib/types';
import type { SessionSnapshot } from '../lib/types';
import { NonModalPanel } from './NonModalPanel';
import type { IntegrationStatus } from '../lib/integrationBridge';
import './shell-log.css';

const labels: Record<CaptureState, string> = { disabled: '未启用', starting: '正在启动（尚未开始记录）', recording: 'Shell 报告记录中', paused: '已暂停', gap: '记录中断 / 存在缺口', disconnected: '连接已断开' };
const localTime = (value: number) => new Date(value).toLocaleString();
const newer = (a: string, b: string) => /^\d+$/.test(a) && /^\d+$/.test(b) ? BigInt(a) >= BigInt(b) : a === b;
export type ShellLogScope = { kind: 'all' } | { kind: 'current' } | { kind: 'profile'; id: string; label: string };
export function ShellLogDialog({ native, snapshot, onClose, initialScope = { kind: 'all' }, onMessage, integration }: { native: boolean; snapshot: SessionSnapshot; onClose: () => void; initialScope?: ShellLogScope; onMessage?: (summary: string) => void; integration?: IntegrationStatus | null }) {
  const [scope, setScope] = useState<ShellLogScope>(initialScope);
  const [scopeSession] = useState(snapshot.session);
  const [scopeTarget] = useState(`${snapshot.username ?? '?'}@${snapshot.host ?? '?'}:${snapshot.port ?? 22}`);
  const [host, setHost] = useState('');
  const [username, setUsername] = useState('');
  const scopeQuery = (selected: ShellLogScope): ShellLogQuery | null => selected.kind === 'current' ? scopeSession ? { session_id: scopeSession.session_id } : null : selected.kind === 'profile' ? { profile_id: selected.id } : {};
  const scopeLabel = scope.kind === 'current' ? `当前终端会话：${scopeSession ? `${scopeTarget} · ${scopeSession.session_id}` : '尚无会话'}` : scope.kind === 'profile' ? `连接历史：${scope.label}（${scope.id}）` : '全部连接与离线记录';
  const [status, setStatus] = useState<ShellLogStatus | null>(null);
  const statusRef = useRef<ShellLogStatus | null>(null);
  const [consent, setConsent] = useState(false);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(false);
  const [uncertain, setUncertain] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [keyword, setKeyword] = useState('');
  const [query, setQuery] = useState<ShellLogQuery>({ limit: 50 });
  const [page, setPage] = useState<ShellLogPage | null>(null);
  const [cursors, setCursors] = useState<(string | null)[]>([null]);
  const [exported, setExported] = useState<ShellLogExport | null>(null);
  const [copied, setCopied] = useState(false);
  const life = useRef(0); const request = useRef(0); const lock = useRef(false);
  const accept = (next: ShellLogStatus) => {
    if (statusRef.current && !newer(next.revision, statusRef.current.revision)) return;
    statusRef.current = next; setStatus(next);
  };
  const load = async (next: ShellLogQuery, trail: (string | null)[]) => {
    const epoch = life.current; const id = ++request.current;
    setLoading(true); setError(null); setExported(null); setPage(null);
    try {
      const result = await shellLogBridge.query(next);
      if (epoch !== life.current || id !== request.current) return;
      setQuery({ ...next, snapshot_max_seq: result.snapshot_max_seq }); setPage(result); setCursors(trail);
    } catch (e) { if (epoch === life.current && id === request.current) setError(safeError(e)); }
    finally { if (epoch === life.current && id === request.current) setLoading(false); }
  };
  useEffect(() => {
    const epoch = life.current; let stop: (() => void) | undefined;
    if (native) {
      void shellLogBridge.listen(next => { if (epoch === life.current) accept(next); }).then(fn => { if (epoch === life.current) stop = fn; else fn(); }).catch(e => { if (epoch === life.current) setError(safeError(e)); });
      void shellLogBridge.status().then(next => { if (epoch === life.current) accept(next); }).catch(e => { if (epoch === life.current) setError(safeError(e)); });
      const filter = scopeQuery(initialScope);
      if (filter) void load({ limit: 50, ...filter }, [null]);
    }
    return () => { life.current++; request.current++; stop?.(); };
  }, [native]);
  useEffect(() => {
    if (!native || status?.state !== 'starting') return;
    let live = true; let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try { const next = await shellLogBridge.status(); if (live) accept(next); }
      catch (e) { if (live) setError(safeError(e)); }
      if (live) timer = setTimeout(() => void poll(), 1000);
    };
    timer = setTimeout(() => void poll(), 1000);
    return () => { live = false; clearTimeout(timer); };
  }, [native, status?.state]);
  useEffect(() => { setConsent(false); }, [snapshot.session?.session_id, snapshot.session?.generation, snapshot.state]);
  const connected = snapshot.state === 'connected' && !!snapshot.session;
  const integrationReady = integration?.phase === 'ready' && sameSession(integration.session, snapshot.session);
  const matches = sameSession(status?.session, snapshot.session);
  const effectiveState = status?.session && (!connected || !matches) ? 'disconnected' : status?.state;
  const startable = native && connected && integrationReady && status?.available && (effectiveState === 'disabled' || (effectiveState === 'disconnected' && !matches));
  const canPause = !!status?.enabled && matches && connected && (effectiveState === 'recording' || effectiveState === 'paused' || effectiveState === 'gap');
  const run = async (operation: () => Promise<void>) => {
    if (lock.current) return;
    lock.current = true; setBusy(true); setError(null);
    const epoch = life.current;
    try { await operation(); } catch (e) { if (epoch === life.current) setError(safeError(e)); }
    finally { lock.current = false; if (epoch === life.current) setBusy(false); }
  };
  const start = () => {
    if (!startable || !snapshot.session || !consent || uncertain) return;
    const session = snapshot.session; const epoch = life.current;
    void run(async () => {
      try { const next = await shellLogBridge.start(session); if (epoch === life.current) { accept(next); setConsent(false); } }
      catch (e) {
        if (epoch === life.current) {
          const code = e && typeof e === 'object' && 'code' in e ? e.code : null;
          // This rejection happens before the native host enables the log consumer.
          // A ready snapshot can expire between the last status read and the click.
          setUncertain(code !== 'SHELL_NOT_READY' && code !== 'SHELL_LOG_CONSENT_REQUIRED');
          setConsent(false);
        }
        throw e;
      }
    });
  };
  const pause = () => {
    if (!canPause) return;
    const epoch = life.current;
    void run(async () => { const next = await shellLogBridge.pause(effectiveState !== 'paused'); if (epoch === life.current) accept(next); });
  };
  const filtersDirty = keyword.trim() !== (query.contains ?? '') || host.trim() !== (query.host ?? '') || username.trim() !== (query.username ?? '');
  const exportFile = (format: ExportFormat) => {
    if (!native || !page || loading || filtersDirty) return;
    const epoch = life.current;
    setExported(null); setCopied(false);
    void run(async () => { const result = await shellLogBridge.export({ ...query, before_seq: null, limit: null }, format); if (epoch === life.current) { setExported(result); onMessage?.(`操作日志已导出：${result.records} 条 ${format.toUpperCase()}，${result.path}`); } });
  };
  return <NonModalPanel title="终端日志" eyebrow="本机记录 · 独立于聊天和模型" onClose={onClose} className="shell-log-dialog">
    <div className="shell-log-body">
      <p>默认关闭。命令、时间和远程工作目录以明文保存在本机，可能包含密码、令牌等秘密；脱敏不能保证识别所有秘密。不保存终端输出，不自动发送给模型。手动与 AI 命令使用同一记录方式。</p>
      {!native && <p role="status">网页预览不提供本机终端日志，请使用桌面应用。</p>}
      <p role="status">记录状态：{effectiveState ? labels[effectiveState] : native ? '正在读取' : '不可用'}{status?.reason && ` · ${status.reason}`}</p>
      {status?.error && <p role="alert" className="inline-error">{status.error.safe_message}</p>}
      {error && <p role="alert" className="inline-error">{error}</p>}
      <details><summary>记录范围与启动说明</summary><p>使用连接时自动准备的当前交互 Bash 集成，不改变工作目录。当前支持 Bash 5.1 / 5.2；请等待工具栏显示“可执行 · Shell 已就绪”，再在此处同意明文记录。只记录该 Shell 顶层命令；嵌套 Shell、不受支持的配置、hook 被修改或断线会造成记录缺口。这不是防篡改审计。暂停只影响日志记录，不关闭执行生命周期集成。</p></details>
      {native && connected && !integrationReady && !status?.enabled && <p className="notice warning" role="status">开始记录前，请等待自动准备完成，工具栏显示“可执行 · Shell 已就绪”。此窗口只开启日志记录，不会重装命令识别；不兼容或状态未知时请查看具体状态。</p>}
      {effectiveState !== 'recording' && effectiveState !== 'paused' && <div className="shell-log-consent">
        <label><input type="checkbox" checked={consent} disabled={!startable || busy} onChange={e => setConsent(e.target.checked)} />我理解本机明文可能包含秘密，同意本次启动记录。</label>
        <button className="button secondary" disabled={!startable || !consent || busy || uncertain} onClick={start}>启动终端日志</button>
      </div>}
      {effectiveState === 'disconnected' && matches && connected && <p>命令状态识别已断开。请核对终端状态；需要时重新连接 SSH 并等待自动准备就绪，再开启日志。</p>}
      {uncertain && <p role="alert">启动结果不确定，已阻止重复提交。请检查终端并关闭、重新打开日志查看状态；不要重复启动。</p>}
      {canPause && <button className="button secondary" disabled={busy || !connected || !matches} onClick={pause}>{effectiveState === 'paused' ? '恢复记录' : '暂停记录'}</button>}
      <div className="shell-log-scope"><label>查看范围<select aria-label="日志查看范围" value={scope.kind} disabled={!native || busy} onChange={e => {
        const selected: ShellLogScope = e.target.value === 'current' ? { kind: 'current' } : e.target.value === 'profile' && initialScope.kind === 'profile' ? initialScope : { kind: 'all' };
        setScope(selected); setKeyword(''); setHost(''); setUsername('');
        const filter = scopeQuery(selected);
        if (filter) void load({ limit: 50, ...filter }, [null]); else { request.current++; setPage(null); setLoading(false); setExported(null); }
      }}><option value="current" disabled={!scopeSession}>当前终端会话</option>{initialScope.kind === 'profile' && <option value="profile">此连接的历史记录</option>}<option value="all">全部记录</option></select></label><p aria-label="当前日志范围">{scopeLabel}</p></div>
      <form className="shell-log-tools" onSubmit={e => { e.preventDefault(); const filter = scopeQuery(scope); if (native && !busy && filter) void load({ limit: 50, ...filter, contains: keyword.trim() || null, ...(host.trim() ? { host: host.trim() } : {}), ...(username.trim() ? { username: username.trim() } : {}) }, [null]); }}>
        <label>原主机<input aria-label="日志原主机" value={host} disabled={!native} onChange={e => setHost(e.target.value)} /></label>
        <label>原账号<input aria-label="日志原账号" value={username} disabled={!native} onChange={e => setUsername(e.target.value)} /></label>
        <label>关键词<input aria-label="日志关键词" value={keyword} disabled={!native} onChange={e => setKeyword(e.target.value)} /></label>
        <button className="button secondary" disabled={!native || busy}>筛选 / 刷新</button>
        <button type="button" className="button secondary" disabled={!page || busy || loading || !native || filtersDirty} onClick={() => exportFile('json')}>导出 JSON</button>
        <button type="button" className="button secondary" disabled={!page || busy || loading || !native || filtersDirty} onClick={() => exportFile('csv')}>导出 CSV</button>
      </form>
      {filtersDirty && <p role="status">筛选条件已修改。请先点击“筛选 / 刷新”，再导出对应结果。</p>}
      <p className="field-hint">当前范围与筛选固定在本次查询快照；刷新才纳入新记录，切换范围不改变记录开关。导出当前筛选的全部记录至本机固定私有目录。CSV 已处理可能触发表格公式的文本；需要规范文本请选择 JSON。</p>
      {exported && <div role="status" className="shell-log-export"><span>已导出 {exported.records} 条（{exported.bytes} 字节）：</span><input aria-label="导出文件完整路径" readOnly value={exported.path} onFocus={e => e.target.select()} /><button className="button secondary" onClick={() => { const epoch = life.current; void Promise.resolve().then(() => navigator.clipboard.writeText(exported.path)).then(() => { if (epoch === life.current) setCopied(true); }).catch(() => { if (epoch === life.current) setError('无法自动复制，请选中完整路径后手动复制。'); }); }}>{copied ? '已复制' : '复制路径'}</button></div>}
      <div aria-label="终端日志记录" aria-busy={loading} className="shell-log-entries">{page?.entries.map(entry => <article key={entry.id}>
        <div>本地收到时间：{localTime(entry.started_at_ms)}{entry.finished_at_ms !== null && ` · 本地结束收到时间：${localTime(entry.finished_at_ms)}`}</div>
        <div>远端报告时间：{entry.remote_started_at === null ? '未提供' : `${entry.remote_started_at} Unix 秒`}（远端时钟未校验）</div>
        <div>{entry.username}@{entry.host}:{entry.port} · {entry.status === 'running' ? '已开始，尚未记录结束' : entry.status === 'completed' ? '前台已返回' : '结果未知'} · 退出码：{entry.exit_code ?? '未知'}{entry.redacted && ' · 已脱敏'}</div>
        <div>原会话：{entry.session_id} / {entry.generation} · {entry.profile_id ? `档案 ID：${entry.profile_id}` : '档案关联未知'}</div>
        <div>远程 cwd（物理路径）：<span className="shell-log-text">{entry.cwd}</span></div>
        <details><summary>完整命令</summary><pre className="shell-log-command">{entry.command}</pre></details>
      </article>)}{page && !page.entries.length && <p>当前筛选下暂无日志。</p>}</div>
      <div className="shell-log-tools"><button className="button secondary" disabled={!native || loading || busy || cursors.length < 2} onClick={() => { const trail = cursors.slice(0, -1); void load({ ...query, before_seq: trail[trail.length - 1] }, trail); }}>上一页</button><span>第 {cursors.length} 页</span><button className="button secondary" disabled={!native || loading || busy || !page?.next_before_seq} onClick={() => { if (page?.next_before_seq) void load({ ...query, before_seq: page.next_before_seq }, [...cursors, page.next_before_seq]); }}>下一页</button></div>
    </div>
  </NonModalPanel>;
}
