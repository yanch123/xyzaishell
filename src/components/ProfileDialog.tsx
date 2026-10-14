import { useEffect, useRef, useState } from 'react';
import { storageBridge } from '../lib/storageBridge';
import type { ProfileStatus, SavedProfile } from '../lib/storageBridge';
import { safeError } from '../lib/types';
import { Modal } from './Modal';
const blank = { id: null as string | null, label: '', host: '', port: 22, username: '', auth_kind: 'password' as SavedProfile['auth_kind'], private_key_path: null as string | null };
export function ProfileDialog({ available, onClose, onConnect }: { available: boolean; onClose: () => void; onConnect: (id: string, revision: string) => Promise<boolean> }) {
  const [status, setStatus] = useState<ProfileStatus | null>(null);
  const [profile, setProfile] = useState(blank);
  const [secret, setSecret] = useState('');
  const [policy, setPolicy] = useState<'keep' | 'save' | 'session'>('keep');
  const [busy, setBusy] = useState(false); const locked = useRef(false);
  const [error, setError] = useState<string | null>(null);
  const [fallback, setFallback] = useState(false);
  const [uncertain, setUncertain] = useState(false);
  useEffect(() => { let live = true; if (available) void storageBridge.profiles().then(value => { if (live) setStatus(value); }).catch(e => { if (live) setError(safeError(e)); }); return () => { live = false; }; }, [available]);
  const run = async (action: () => Promise<void>) => { if (locked.current) return; locked.current = true; setBusy(true); setError(null); try { await action(); } catch (e) { setError(safeError(e)); setUncertain(true); } finally { setSecret(''); locked.current = false; setBusy(false); } };
  const pick = (value?: SavedProfile) => { setProfile(value ? { id: value.id, label: value.label, host: value.host, port: value.port, username: value.username, auth_kind: value.auth_kind, private_key_path: value.private_key_path } : blank); setSecret(''); setPolicy('keep'); setError(null); };
  const savedProfile = status?.listing.connections.find(item => item.id === profile.id);
  const edited = !savedProfile || (['label', 'host', 'port', 'username', 'auth_kind', 'private_key_path'] as const).some(key => profile[key] !== savedProfile[key]) || policy !== 'keep';
  const save = () => {
    if (!status || locked.current || uncertain) return;
    if (!profile.label.trim() || !profile.host.trim() || /[\s/\\]/u.test(profile.host) || !profile.username.trim() || !Number.isInteger(profile.port) || profile.port < 1 || profile.port > 65535) { setError('请填写档案名称、有效主机、用户名和端口。'); return; }
    if (profile.auth_kind === 'private_key' && !/^(\/|[a-zA-Z]:[\\/]|\\\\)/u.test(profile.private_key_path ?? '')) { setError('私钥需使用本机绝对路径。'); return; }
    return run(async () => {
    const value = secret; setSecret('');
    const next = await storageBridge.saveProfile({ expected_revision: status.listing.revision, profile: { ...profile, label: profile.label.trim(), host: profile.host.trim(), username: profile.username.trim(), private_key_path: profile.auth_kind === 'private_key' ? profile.private_key_path : null }, credential_update: policy === 'save' ? { kind: 'set', value } : { kind: 'keep' } });
    setStatus(next);
    const saved = next.listing.connections.find(item => profile.id ? item.id === profile.id : !status.listing.connections.some(old => old.id === item.id));
    if (saved) { setProfile({ id: saved.id, label: saved.label, host: saved.host, port: saved.port, username: saved.username, auth_kind: saved.auth_kind, private_key_path: saved.private_key_path }); if (policy === 'session') setStatus(await storageBridge.sessionCredential(saved.id, next.listing.revision, value || null)); }
    setPolicy('keep');
    });
  };
  return <Modal title="已保存的 SSH 档案" eyebrow="LOCAL CONNECTION PROFILES" onClose={() => { if (!busy) { setSecret(''); onClose(); } }}>
    <p className="modal-intro">档案保存主机和用户名。只有明确选择“保存凭据”才持久保存密码或口令；不会发送给模型。临时连接可返回使用“仅本次连接”表单，无需保存档案。</p>
    {!available && <p className="notice">网页预览不能读取或保存本机档案。</p>}
    <button className="button secondary" disabled={!available || busy} onClick={() => void run(async () => { const next = await storageBridge.profiles(); setStatus(next); setUncertain(false); pick(); })}>重新读取档案状态</button>
    {uncertain && <p className="notice warning" role="alert">保存结果可能不确定，已阻止再次提交。请重新读取档案状态并检查是否已保存，再明确选择要编辑的档案。</p>}
    {status?.path_warning && <p className="notice warning">{status.path_warning}</p>}
    {status && <><div className="notice">凭据后端：{status.credentials.credential_backend} · {status.credentials.credential_availability}<br />配置目录：{status.configuration_root ?? '不可用'}</div>
      <label>选择档案<select aria-label="选择档案" value={profile.id ?? ''} disabled={busy} onChange={e => pick(status.listing.connections.find(item => item.id === e.target.value))}><option value="">新建档案</option>{status.listing.connections.map(item => <option key={item.id} value={item.id}>{item.label} · {item.username}@{item.host}</option>)}</select></label>
      {savedProfile && <p className="field-hint">凭据状态：{savedProfile.credential_present ? '已提供（不显示内容）' : '未提供，密码认证前需要填写'}</p>}
      <fieldset disabled={busy}><label>档案名称<input value={profile.label} onChange={e => setProfile({ ...profile, label: e.target.value })} maxLength={128} /></label><div className="form-row"><label className="grow">档案主机<input value={profile.host} onChange={e => setProfile({ ...profile, host: e.target.value })} maxLength={253} /></label><label className="port-field">档案端口<input type="number" min={1} max={65535} value={profile.port} onChange={e => setProfile({ ...profile, port: Number(e.target.value) })} /></label></div><label>档案用户名<input value={profile.username} onChange={e => setProfile({ ...profile, username: e.target.value })} maxLength={128} /></label>
      <label>档案认证方式<select value={profile.auth_kind} onChange={e => { setProfile({ ...profile, auth_kind: e.target.value as SavedProfile['auth_kind'], private_key_path: null }); setSecret(''); setPolicy('keep'); }}><option value="password">密码</option><option value="private_key">私钥文件</option></select></label>
      {profile.auth_kind === 'private_key' && <label>私钥绝对路径<input value={profile.private_key_path ?? ''} onChange={e => setProfile({ ...profile, private_key_path: e.target.value })} /></label>}
      <label>凭据处理<select aria-label="凭据处理" value={policy} onChange={e => { setPolicy(e.target.value as typeof policy); setSecret(''); }}><option value="keep">不更改凭据（新档案不保存）</option>{status.listing.mode === 'session_only' && <option value="session">仅本次进程使用凭据</option>}<option value="save" disabled={!status.credentials.persistent_credentials || status.credentials.credential_availability !== 'available'}>明确保存凭据到系统安全存储</option></select></label>
      {policy !== 'keep' && <label>{profile.auth_kind === 'password' ? '档案密码' : '档案私钥口令'}<input type="password" autoComplete="new-password" value={secret} onChange={e => setSecret(e.target.value)} maxLength={4096} /></label>}
      <p className="field-hint">保存失败不会自动退回明文。凭据输入在提交后清除；保存与连接是独立操作。编辑后请先保存，再连接。</p></fieldset>
      {status.listing.mode !== 'session_only' && <div className="notice warning"><label className="check-field"><input type="checkbox" checked={fallback} disabled={busy} onChange={e => setFallback(e.target.checked)} /><span>将全部 {status.listing.connections.length} 个档案切换为会话内存模式，移除其持久凭据引用，需要重新输入凭据。当前界面不能切回系统安全存储模式，请确认后再切换。</span></label><button className="button secondary" disabled={!fallback || busy || uncertain} onClick={() => void run(async () => { setStatus(await storageBridge.sessionMode(status.listing.revision)); setFallback(false); })}>确认切换全部档案</button></div>}
    </>}
    {error && <p className="inline-error" role="alert">{error}</p>}
    <div className="modal-footer"><button className="button secondary" disabled={busy} onClick={onClose}>返回</button><button className="button secondary" disabled={!status || busy || uncertain} onClick={() => void save()}>保存档案</button><button className="button primary" disabled={!status || !profile.id || busy || uncertain || edited} onClick={() => void run(async () => { if (status && profile.id) { if (await onConnect(profile.id, status.listing.revision)) onClose(); else setError('连接请求未成功。请返回工作区核对连接状态，再明确发起连接。'); } })}>连接已保存档案</button></div>
  </Modal>;
}
