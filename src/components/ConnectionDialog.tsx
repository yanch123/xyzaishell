import { useState } from 'react';
import type { FormEvent } from 'react';
import type { ConnectRequest, TerminalSize } from '../lib/types';
import { Icon } from './Icon';
import { Modal } from './Modal';
export function ConnectionDialog({ onClose, onConnect, size, available }: { onClose: () => void; onConnect: (request: ConnectRequest) => Promise<boolean>; size: TerminalSize; available: boolean }) {
  const [host, setHost] = useState(''); const [port, setPort] = useState('22'); const [username, setUsername] = useState('');
  const [kind, setKind] = useState<'password' | 'private_key'>('password');
  const [password, setPassword] = useState(''); const [path, setPath] = useState(''); const [passphrase, setPassphrase] = useState('');
  const [sessionOnly, setSessionOnly] = useState(false); const [busy, setBusy] = useState(false); const [error, setError] = useState<string | null>(null);
  const close = () => { if (!busy) { setPassword(''); setPassphrase(''); onClose(); } };
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (busy || !available) return;
    const p = Number(port);
    if (!host.trim() || /[\s/\\]/u.test(host.trim()) || !username.trim() || !Number.isInteger(p) || p < 1 || p > 65535) { setError('请填写有效主机、用户名和 1–65535 之间的端口。'); return; }
    if (!sessionOnly) { setError('请确认仅为本次连接提供凭据。'); return; }
    if (kind === 'private_key' && !/^(\/|[a-zA-Z]:[\\/]|\\\\)/u.test(path.trim())) { setError('私钥需使用这台电脑上的绝对路径。'); return; }
    setError(null); setBusy(true);
    const request: ConnectRequest = { host: host.trim(), port: p, username: username.trim(), session_only: true, size,
      auth: kind === 'password' ? { kind, password } : { kind, private_key_path: path.trim(), ...(passphrase ? { passphrase } : {}) } };
    // No credential retention after the request is handed to the native host, including errors.
    setPassword(''); setPassphrase('');
    const success = await onConnect(request);
    setBusy(false);
    if (success) onClose(); else setError('连接请求未成功。请查看主界面提示，检查后重新输入凭据。');
  };
  return <Modal title="连接 SSH 服务器" eyebrow="NEW CONNECTION" onClose={close}>
    <p className="modal-intro">一个窗口，连接一台服务器。新的连接会创建独立的 SSH 终端会话。</p>
    {!available && <div className="notice warning"><Icon name="info" /><span>网页预览无法连接 SSH。请通过 Tauri 桌面应用启动。</span></div>}
    <form onSubmit={event => void submit(event)} autoComplete="off">
      <fieldset disabled={busy}>
        <div className="form-row"><label className="grow">主机地址<input aria-label="主机地址" value={host} onChange={e => setHost(e.target.value)} placeholder="192.168.1.10 或 server.example.com" maxLength={253} required spellCheck={false} autoCapitalize="none" /></label><label className="port-field">端口<input aria-label="端口" value={port} onChange={e => setPort(e.target.value)} inputMode="numeric" maxLength={5} required /></label></div>
        <label>用户名<input value={username} onChange={e => setUsername(e.target.value)} placeholder="如 ubuntu" maxLength={128} required spellCheck={false} autoCapitalize="none" /></label>
        <div className="field-label">认证方式</div><div className="segmented" role="group" aria-label="认证方式"><button type="button" aria-pressed={kind === 'password'} className={kind === 'password' ? 'selected' : ''} onClick={() => { setKind('password'); setPassphrase(''); }}><Icon name="lock" size={16} />密码</button><button type="button" aria-pressed={kind === 'private_key'} className={kind === 'private_key' ? 'selected' : ''} onClick={() => { setKind('private_key'); setPassword(''); }}><Icon name="key" size={16} />私钥文件</button></div>
        {kind === 'password' ? <label>登录密码<input value={password} onChange={e => setPassword(e.target.value)} type="password" autoComplete="new-password" maxLength={4096} placeholder="仅用于本次 SSH 认证" required /></label> : <><div className="notice warning"><Icon name="info" size={15} /><span>M1 私钥登录仅支持 Linux；Windows 请使用密码认证。私钥路径、格式与权限由本机宿主校验。</span></div><label>本机私钥绝对路径<input value={path} onChange={e => setPath(e.target.value)} placeholder="/home/user/.ssh/id_ed25519 或 C:\Users\user\.ssh\id_ed25519" maxLength={4096} required spellCheck={false} /></label><label>私钥口令 <span className="optional">可选</span><input type="password" value={passphrase} onChange={e => setPassphrase(e.target.value)} maxLength={4096} autoComplete="new-password" placeholder="仅在私钥已加密时填写" /></label></>}
        <label className="check-field"><input type="checkbox" checked={sessionOnly} onChange={e => setSessionOnly(e.target.checked)} /><span>仅本次连接使用凭据，不保存密码或私钥口令<div className="field-hint">M1 使用临时认证；重连需要重新填写。不会将凭据发送给 AI。</div></span></label>
      </fieldset>
      {error && <p className="inline-error" role="alert">{error}</p>}
      <div className="modal-footer"><span className="security-note"><Icon name="shield" size={15} />首次连接需核验主机指纹</span><button className="button secondary" type="button" onClick={close} disabled={busy}>取消</button><button className="button primary" type="submit" disabled={busy || !available || !sessionOnly}>{busy ? '正在提交…' : '连接服务器'}<Icon name="arrow" size={15} /></button></div>
    </form>
  </Modal>;
}
