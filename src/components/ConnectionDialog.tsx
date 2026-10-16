import { useRef, useState } from 'react';
import type { FormEvent } from 'react';
import type { ConnectRequest, TerminalSize } from '../lib/types';
import { safeError } from '../lib/types';
import { storageBridge } from '../lib/storageBridge';
import type { ConnectSavedProfile, ProfileStatus, SavedProfile } from '../lib/storageBridge';
import { Icon } from './Icon';
import { Modal } from './Modal';
import { NonModalPanel } from './NonModalPanel';

type Props = {
  onClose: () => void; onConnect: (request: ConnectRequest) => Promise<boolean>;
  onConnectSaved?: ConnectSavedProfile; size: TerminalSize; available: boolean; onProfiles?: () => void;
  profile?: SavedProfile; status?: ProfileStatus; onSaved?: (status: ProfileStatus) => void;
  initialTarget?: Pick<SavedProfile, 'host' | 'port' | 'username' | 'auth_kind'> & { private_key_path?: string | null };
};
/** One editor for temporary connections, save-and-connect, and metadata edits. */
export function ConnectionDialog({ onClose, onConnect, onConnectSaved, size, available, onProfiles, profile, status: initialStatus, onSaved, initialTarget }: Props) {
  const initial = profile ?? initialTarget;
  const [label, setLabel] = useState(profile?.label ?? '');
  const [host, setHost] = useState(initial?.host ?? '');
  const [port, setPort] = useState(String(initial?.port ?? 22));
  const [username, setUsername] = useState(initial?.username ?? '');
  const [kind, setKind] = useState<'password' | 'private_key'>(initial?.auth_kind ?? 'password');
  const [path, setPath] = useState(initial?.private_key_path ?? '');
  const [secret, setSecret] = useState('');
  const [saveConnection, setSaveConnection] = useState(!!profile);
  const [remember, setRemember] = useState(profile?.credential_policy === 'system_secure');
  const [removeConfirmed, setRemoveConfirmed] = useState(false);
  const [status, setStatus] = useState(initialStatus);
  const [busy, setBusy] = useState(false); const locked = useRef(false);
  const [phase, setPhase] = useState(''); const [error, setError] = useState<string | null>(null);
  const [uncertain, setUncertain] = useState(false);
  const [discarding, setDiscarding] = useState(false);
  const saved = useRef<{ id: string; revision: string; fingerprint: string } | null>(null);
  const id = useRef(profile?.id ?? crypto.randomUUID());
  const dirty = secret !== '' || (profile ? label !== profile.label || host !== profile.host || port !== String(profile.port) || username !== profile.username || kind !== profile.auth_kind || path !== (profile.private_key_path ?? '') || remember !== (profile.credential_policy === 'system_secure') : !!(label || host || username || path));
  const close = () => {
    if (locked.current) return;
    if (dirty) { setDiscarding(true); return; }
    setSecret(''); onClose();
  };
  const submit = async (event: Pick<FormEvent, 'preventDefault'>, saveOnly = false) => {
    event.preventDefault();
    if (locked.current || !available || uncertain) return;
    const p = Number(port);
    if (!host.trim() || /[\s/\\]/u.test(host.trim()) || !username.trim() || !Number.isInteger(p) || p < 1 || p > 65535) { setError('请填写有效主机、用户名和 1–65535 之间的端口。'); return; }
    if (saveConnection && !label.trim()) { setError('保存连接时请填写名称。'); return; }
    if (kind === 'private_key' && !/^(\/|[a-zA-Z]:[\\/])/u.test(path.trim())) { setError('私钥需使用这台电脑上的绝对路径。'); return; }
    const unchangedIdentity = !!profile && host.trim() === profile.host && p === profile.port && username.trim() === profile.username && kind === profile.auth_kind && (kind === 'password' || path.trim() === profile.private_key_path);
    const keep = remember && !!profile?.credential_present && !secret;
    if (keep && !unchangedIdentity) { setError('认证目标已改变。请为新目标输入凭据，或取消记住并明确移除原凭据。'); return; }
    if (profile?.credential_present && !remember && !removeConfirmed) { setError('请明确确认只移除此档案的已保存凭据。'); return; }
    if (remember && !secret && !keep && !saved.current) { setError(kind === 'password' ? '记住密码需要输入密码。' : '记住私钥口令需要输入口令；无口令私钥无需勾选。'); return; }
    if (!profile && !saveOnly && kind === 'password' && !secret && !(remember && saved.current)) { setError('请输入本次连接密码。'); return; }
    locked.current = true; setBusy(true); setError(null);
    const value = secret; setSecret('');
    const draft = { id: id.current, label: label.trim(), host: host.trim(), port: p, username: username.trim(), auth_kind: kind, private_key_path: kind === 'private_key' ? path.trim() : null };
    const fingerprint = JSON.stringify({ ...draft, remember });
    let didSave = false;
    try {
      let savedProfile = saved.current?.fingerprint === fingerprint && !(remember && value) ? saved.current : null;
      if (saveConnection && !savedProfile) {
        setPhase('正在保存连接…');
        const current = status ?? await storageBridge.profiles(); setStatus(current);
        const next = await storageBridge.saveProfile({ expected_revision: current.listing.revision, profile: draft,
          credential_update: remember ? (value ? { kind: 'set', value } : { kind: 'keep' }) : profile?.credential_present ? { kind: 'clear' } : { kind: 'keep' } });
        setStatus(next); onSaved?.(next); didSave = true;
        savedProfile = { id: id.current, revision: next.listing.revision, fingerprint }; saved.current = savedProfile;
        if (profile || saveOnly) { setPhase('连接资料已保存。'); onClose(); return; }
        setPhase('连接已保存，正在连接…');
      } else { setPhase(savedProfile ? '连接已保存，正在连接…' : '正在连接…'); }
      if (saveOnly) { onClose(); return; }
      const success = savedProfile && onConnectSaved
        ? await onConnectSaved(savedProfile.id, savedProfile.revision, !remember && value ? { temporaryCredential: value } : undefined)
        : await onConnect({ host: draft.host, port: p, username: draft.username, session_only: true, size,
          auth: kind === 'password' ? { kind, password: value } : { kind, private_key_path: path.trim(), ...(value ? { passphrase: value } : {}) } });
      if (success) onClose();
      else { setPhase(savedProfile ? '连接已保存，连接请求未成功。' : '连接请求未成功。'); setError('请核对连接状态后重新输入凭据，再明确连接。'); }
    } catch (failure) {
      setError(safeError(failure));
      const code = failure && typeof failure === 'object' && 'code' in failure ? failure.code : null;
      if (saveConnection && !didSave && !saved.current && (code === 'CONFIG_SAVE_UNCERTAIN' || code === 'CONFIG_CONFLICT' || !code)) setUncertain(true);
      setPhase(didSave || saved.current ? '连接已保存；连接未完成。' : '连接未保存；未发起连接。');
    } finally { locked.current = false; setBusy(false); }
  };
  const Container = profile ? NonModalPanel : Modal;
  return <Container title={profile ? '编辑 SSH 连接' : '连接 SSH 服务器'} eyebrow={profile ? 'EDIT CONNECTION' : 'NEW CONNECTION'} onClose={close}>
    <p className="modal-intro">{profile ? '保存只更新此连接资料，不改变已建立的会话。' : '保存连接和记住凭据可以分别选择。凭据仅用于 SSH 认证。'}</p>
    {!profile && <p className="connection-shell-notice">连接后会在当前终端自动启动 Bash 5.1 / 5.2 并读取 ~/.bashrc，临时准备命令执行。使用非登录 Shell，不读取登录配置文件；不修改远程配置。不兼容或启动需要交互时，可继续使用手动终端。</p>}
    {!available && <p className="notice warning">网页预览无法连接 SSH。请通过桌面应用启动。</p>}
    {!profile && onProfiles && <button className="button secondary" onClick={onProfiles} disabled={busy}>已保存连接</button>}
    {discarding && <div className="notice warning" role="alert"><p>放弃尚未保存的连接修改？本次输入的密码或口令会清除。</p><button className="button secondary" type="button" onClick={() => setDiscarding(false)}>继续编辑</button><button className="button danger" type="button" onClick={() => { setSecret(''); onClose(); }}>放弃修改</button></div>}
    <form onSubmit={event => void submit(event)} autoComplete="off">
      <fieldset disabled={busy}>
        <label>连接名称{!saveConnection && <span className="optional"> 可选</span>}<input aria-label="连接名称" value={label} onChange={e => setLabel(e.target.value)} maxLength={128} /></label>
        <div className="form-row"><label className="grow">主机地址<input aria-label="主机地址" value={host} onChange={e => setHost(e.target.value)} maxLength={253} required spellCheck={false} autoCapitalize="none" /></label><label className="port-field">端口<input aria-label="端口" value={port} onChange={e => setPort(e.target.value)} inputMode="numeric" maxLength={5} required /></label></div>
        <label>用户名<input value={username} onChange={e => setUsername(e.target.value)} maxLength={128} required spellCheck={false} autoCapitalize="none" /></label>
        <div className="segmented" role="group" aria-label="认证方式"><button type="button" aria-pressed={kind === 'password'} onClick={() => { setKind('password'); setSecret(''); }}>密码</button><button type="button" aria-pressed={kind === 'private_key'} onClick={() => { setKind('private_key'); setSecret(''); }}>私钥文件</button></div>
        {kind === 'private_key' && <label>本机私钥绝对路径<input value={path} onChange={e => setPath(e.target.value)} maxLength={4096} required spellCheck={false} /></label>}
        <label>{kind === 'password' ? '登录密码' : '私钥口令'}<input value={secret} onChange={e => setSecret(e.target.value)} type="password" autoComplete="new-password" maxLength={4096} placeholder={profile?.credential_present && remember ? '已保存，留空保留' : kind === 'private_key' ? '仅在私钥已加密时填写' : '仅用于本次 SSH 认证'} /></label>
        {!profile && <label className="check-field"><input type="checkbox" checked={saveConnection} onChange={e => { setSaveConnection(e.target.checked); if (!e.target.checked) setRemember(false); }} /><span>保存连接</span></label>}
        <label className="check-field"><input type="checkbox" checked={remember} disabled={!saveConnection} onChange={e => { setRemember(e.target.checked); setRemoveConfirmed(false); }} /><span>{kind === 'password' ? '记住密码' : '记住私钥口令'}<div className="field-hint">仅保存在本机，使用系统保护；本机账号或系统被控制时仍有风险。</div></span></label>
        {profile?.credential_present && !remember && <label className="check-field"><input type="checkbox" checked={removeConfirmed} onChange={e => setRemoveConfirmed(e.target.checked)} /><span>确认移除此连接的凭据引用，之后按需询问。旧版备份或系统保险库中的条目可能保留。</span></label>}
        {status?.credentials.credential_availability !== undefined && status.credentials.credential_availability !== 'available' && <p className="notice warning">系统凭据存储暂不可用（{status.credentials.credential_availability}）。可以取消记住后保存资料并临时连接；不会自动降低其他连接的保护。</p>}
      </fieldset>
      {phase && <p role="status">{phase}</p>}
      {error && <p className="inline-error" role="alert">{error}</p>}
      {uncertain && <><p className="notice warning">保存结果需要核对。请重新读取；不会自动重试或连接。</p><button type="button" className="button secondary" disabled={busy} onClick={() => void storageBridge.profiles().then(next => { setStatus(next); setUncertain(false); saved.current = null; setError(null); onSaved?.(next); }).catch(e => setError(safeError(e)))}>重新读取档案状态</button></>}
      <div className="modal-footer"><span className="security-note"><Icon name="shield" size={15} />首次连接需核验主机指纹</span><button className="button secondary" type="button" onClick={close} disabled={busy}>取消</button>{!profile && saveConnection && <button className="button secondary" type="button" disabled={busy || !available || uncertain} onClick={event => void submit(event, true)}>仅保存连接</button>}<button className="button primary" type="submit" disabled={busy || !available || uncertain}>{busy ? '正在提交…' : profile ? '保存' : '连接服务器'}</button></div>
    </form>
  </Container>;
}
