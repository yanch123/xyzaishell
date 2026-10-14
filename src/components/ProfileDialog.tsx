import { useEffect, useRef, useState } from 'react';
import { storageBridge } from '../lib/storageBridge';
import type { ConnectSavedProfile, ProfileConnectOptions, ProfileStatus, SavedProfile } from '../lib/storageBridge';
import { safeError } from '../lib/types';
import { NonModalPanel } from './NonModalPanel';
import { ConnectionDialog } from './ConnectionDialog';

type Props = { available: boolean; onClose: () => void; onConnect: ConnectSavedProfile; initialProfileId?: string;
  activeSession?: boolean; onViewLogs?: (profile: SavedProfile) => void; onConnectInNewWindow?: (id: string) => Promise<unknown> | void;
  onSwitchCurrent?: ConnectSavedProfile; onNewConnection?: () => void;
};
export function ProfileDialog({ available, onClose, onConnect, initialProfileId, activeSession, onViewLogs, onConnectInNewWindow, onSwitchCurrent, onNewConnection }: Props) {
  const [status, setStatus] = useState<ProfileStatus | null>(null);
  const [selected, setSelected] = useState(initialProfileId ?? '');
  const appliedInitialId = useRef(initialProfileId);
  const [editing, setEditing] = useState<SavedProfile | null>(null);
  const [busy, setBusy] = useState(false); const locked = useRef(false); const live = useRef(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState('');
  const [deleting, setDeleting] = useState(false); const [uncertain, setUncertain] = useState(false);
  const [challenge, setChallenge] = useState<{ profile: SavedProfile; revision: string; switchCurrent: boolean } | null>(null);
  const [secret, setSecret] = useState(''); const [remember, setRemember] = useState(false);
  const refresh = async () => { const next = await storageBridge.profiles(); if (live.current) { setStatus(next); setUncertain(false); } return next; };
  useEffect(() => {
    live.current = true;
    if (available) void refresh().catch(e => { if (live.current) setError(safeError(e)); });
    return () => { live.current = false; };
  }, [available]);
  useEffect(() => {
    if (initialProfileId === appliedInitialId.current || !initialProfileId) return;
    if (editing || busy) return; // Preserve an active editor/attempt until its guarded close.
    appliedInitialId.current = initialProfileId;
    setSelected(initialProfileId); setChallenge(null); setSecret(''); setDeleting(false);
    if (available) void refresh().catch(e => { if (live.current) setError(safeError(e)); });
  }, [initialProfileId, editing, busy, available]);
  const profile = status?.listing.connections.find(item => item.id === selected);
  const run = async (work: () => Promise<void>) => {
    if (locked.current) return;
    locked.current = true; setBusy(true); setError(null);
    try { await work(); } catch (e) { if (live.current) setError(safeError(e)); }
    finally { locked.current = false; if (live.current) setBusy(false); }
  };
  const invokeConnect = async (item: SavedProfile, revision: string, switchCurrent: boolean, options?: ProfileConnectOptions) => {
    try {
      const success = await (switchCurrent && onSwitchCurrent ? onSwitchCurrent : onConnect)(item.id, revision, options);
      if (!live.current) return;
      if (success) { setChallenge(null); onClose(); }
      else setError('连接请求未成功。请核对连接状态后再明确重试。');
    } catch (e) {
      const code = e && typeof e === 'object' && 'code' in e ? e.code : '';
      if (typeof code === 'string' && ['CREDENTIAL_MISSING', 'CREDENTIAL_LOCKED', 'CREDENTIAL_UNAVAILABLE', 'CREDENTIAL_ACCESS_DENIED', 'CREDENTIAL_BACKEND_MISMATCH', 'CREDENTIAL_PASSPHRASE_REQUIRED'].includes(code)) {
        setChallenge({ profile: item, revision, switchCurrent }); setRemember(false);
      }
      setError(safeError(e));
    }
  };
  const connect = (switchCurrent = false) => run(async () => {
    if (!profile) return;
    if (activeSession && !switchCurrent && onConnectInNewWindow) { await onConnectInNewWindow(profile.id); return; }
    const next = await refresh(); const item = next.listing.connections.find(p => p.id === profile.id);
    if (!item) { setError('此连接已被其他窗口删除，请重新选择。'); return; }
    if (item.host !== profile.host || item.port !== profile.port || item.username !== profile.username || item.auth_kind !== profile.auth_kind || item.private_key_path !== profile.private_key_path) {
      setError('此连接的认证目标已被其他窗口修改。请核对当前显示的资料后再次连接。'); return;
    }
    if ((item.auth_kind === 'password' && !item.credential_present) || (item.credential_present && next.credentials.credential_availability !== 'available')) {
      setChallenge({ profile: item, revision: next.listing.revision, switchCurrent }); setRemember(false); setSecret(''); return;
    }
    await invokeConnect(item, next.listing.revision, switchCurrent);
  });
  const close = () => { if (!locked.current) { setSecret(''); setChallenge(null); onClose(); } };
  if (editing && status) return <ConnectionDialog profile={editing} status={status} available={available} size={{ cols: 100, rows: 30 }} onConnect={async () => false} onSaved={next => { setStatus(next); setNotice('连接资料已保存。'); }} onClose={() => setEditing(null)} />;
  return <NonModalPanel title="已保存的 SSH 连接" eyebrow="SAVED CONNECTIONS" onClose={close}>
    {!available && <p className="notice">网页预览不能读取或保存本机连接。</p>}
    <div className="button-row"><button className="button secondary" disabled={!available || busy} onClick={() => void run(async () => { await refresh(); setChallenge(null); setSecret(''); })}>重新读取档案状态</button>{onNewConnection && <button className="button secondary" disabled={busy} onClick={onNewConnection}>新建连接</button>}</div>
    {status?.path_warning && <p className="notice warning">{status.path_warning}</p>}
    {status && <>
      <label>选择档案<select aria-label="选择档案" value={selected} disabled={busy} onChange={e => { setSelected(e.target.value); setChallenge(null); setSecret(''); setDeleting(false); setError(null); }}><option value="">请选择连接</option>{status.listing.connections.map(item => <option key={item.id} value={item.id} data-profile-id={item.id}>{item.label} · {item.username}@{item.host}:{item.port}</option>)}</select></label>
      {profile && <div data-profile-id={profile.id} onDoubleClick={() => { if (!challenge) void connect(); }}><p>{profile.username}@{profile.host}:{profile.port} · {profile.auth_kind === 'password' ? '密码' : '私钥'}</p><p className="field-hint">凭据：{profile.credential_policy === 'system_secure' ? profile.credential_present ? status.credentials.credential_availability === 'available' ? '已保存，连接时读取系统保护凭据' : '已保存，系统存储暂不可用' : '系统保护已选择，但凭据缺失' : '按需询问，不保存密码或口令'}</p></div>}
      {status.credentials.credential_availability !== 'available' && <p className="notice warning">系统凭据存储：{status.credentials.credential_availability}。原有引用保留，可在连接时明确临时输入；资料编辑仍可用。</p>}
      <div className="button-row"><button className="button secondary" disabled={!profile || busy || !!challenge} onClick={() => { if (profile) setEditing(profile); }}>编辑 / 重命名</button><button className="button secondary" disabled={!profile || busy || uncertain || !!challenge} onClick={() => setDeleting(true)}>删除连接</button>{onViewLogs && <button className="button secondary" disabled={!profile || busy} onClick={() => { if (profile) onViewLogs(profile); }}>操作日志</button>}</div>
      {deleting && profile && <div className="notice warning"><p>删除“{profile.label}”的连接资料和当前凭据引用？不会删除私钥文件、主机信任、聊天或操作日志，也不会断开当前会话。系统保险库或受限旧版备份中的孤立条目暂予保留。</p><button className="button secondary" disabled={busy} onClick={() => setDeleting(false)}>取消删除</button><button className="button danger" disabled={busy} onClick={() => void run(async () => { try { const next = await storageBridge.deleteProfile(profile.id, status.listing.revision); setStatus(next); setSelected(''); setDeleting(false); setNotice('连接资料已删除。'); } catch (e) { setUncertain(true); throw e; } })}>确认删除此连接</button></div>}
    </>}
    {challenge && <form autoComplete="off" onSubmit={event => { event.preventDefault(); if (!secret || locked.current || uncertain) return; const value = secret; setSecret(''); void run(async () => {
        if (!remember) { await invokeConnect(challenge.profile, challenge.revision, challenge.switchCurrent, { temporaryCredential: value, rememberCredential: false }); return; }
        try {
          const { credential_present: _present, credential_policy: _policy, ...draft } = challenge.profile;
          const next = await storageBridge.saveProfile({ expected_revision: challenge.revision, profile: draft, credential_update: { kind: 'set', value } });
          setStatus(next); setNotice('此连接的凭据已保存；正在连接。');
          setChallenge({ ...challenge, revision: next.listing.revision });
          await invokeConnect(next.listing.connections.find(item => item.id === draft.id) ?? challenge.profile, next.listing.revision, challenge.switchCurrent);
        } catch (failure) {
          const code = failure && typeof failure === 'object' && 'code' in failure ? failure.code : null;
          if (!code || code === 'CONFIG_CONFLICT' || code === 'CONFIG_SAVE_UNCERTAIN') setUncertain(true);
          throw failure;
        }
      }); }}>
      <p>为 {challenge.profile.username}@{challenge.profile.host}:{challenge.profile.port} 提供本次{challenge.profile.auth_kind === 'password' ? '密码' : '私钥口令'}。</p>
      <label>{challenge.profile.auth_kind === 'password' ? '本次密码' : '本次私钥口令'}<input type="password" autoComplete="new-password" maxLength={4096} value={secret} onChange={e => setSecret(e.target.value)} disabled={busy} required /></label>
      <label className="check-field"><input type="checkbox" checked={remember} disabled={busy || status?.credentials.credential_availability !== 'available'} onChange={e => setRemember(e.target.checked)} /><span>为此连接记住凭据，使用本机系统保护</span></label>
      <button className="button secondary" type="button" disabled={busy} onClick={() => { setChallenge(null); setSecret(''); setError(null); }}>取消认证</button><button className="button primary" type="submit" disabled={busy || !secret || uncertain}>{remember ? '保存凭据并连接' : '仅本次输入连接'}</button>
    </form>}
    {notice && <p role="status">{notice}</p>}
    {error && <p className="inline-error" role="alert">{error}</p>}
    {uncertain && <p className="notice warning">结果需要核对，请重新读取后再操作。</p>}
    <div className="modal-footer"><button className="button secondary" disabled={busy} onClick={close}>关闭</button>{activeSession && onSwitchCurrent && <button className="button secondary" disabled={!profile || busy || !!challenge} onClick={() => void connect(true)}>在当前窗口切换</button>}<button className="button primary" disabled={!profile || busy || !!challenge} onClick={() => void connect()}>{activeSession ? '在新窗口连接' : '连接已保存档案'}</button></div>
  </NonModalPanel>;
}
