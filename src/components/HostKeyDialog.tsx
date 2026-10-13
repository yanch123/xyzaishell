import { useState } from 'react';
import type { HostKeyChallenge } from '../lib/types';
import { safeError } from '../lib/types';
import { Icon } from './Icon';
import { Modal } from './Modal';
export function HostKeyDialog({ challenge, onDecide }: { challenge: HostKeyChallenge; onDecide: (challenge: HostKeyChallenge, accept: boolean) => Promise<void> }) {
  const [verified, setVerified] = useState(false); const [busy, setBusy] = useState(false); const [error, setError] = useState<string | null>(null);
  const decide = async (accept: boolean) => {
    if (busy || (accept && !verified)) return;
    setBusy(true);
    try { await onDecide(challenge, accept); }
    catch (failure) { setError(safeError(failure)); }
    finally { setBusy(false); }
  };
  return <Modal title={challenge.changed ? '警告：服务器身份已变化' : '核验服务器身份'} eyebrow="HOST KEY VERIFICATION" onClose={() => { if (!busy) void decide(false); }}>
    <div className={`notice ${challenge.changed ? 'danger' : 'warning'}`}><Icon name="shield" /><span>{challenge.changed ? '主机密钥与本进程已信任的密钥不同。可能是服务器重装，也可能存在中间人攻击。请通过可信渠道核对后再继续。' : '此进程尚未信任这台服务器。请向服务器管理员核对以下 SHA-256 指纹。'}</span></div>
    <dl className="host-details"><div><dt>服务器</dt><dd>{challenge.host}:{challenge.port}</dd></div><div><dt>算法</dt><dd>{challenge.algorithm}</dd></div></dl>
    {challenge.previous_fingerprint && <div className="fingerprint old"><span>上次信任的指纹</span><code>{challenge.previous_fingerprint}</code></div>}
    <div className="fingerprint"><span>{challenge.changed ? '当前新指纹' : '服务器指纹'}</span><code>{challenge.fingerprint}</code></div>
    <label className="check-field"><input type="checkbox" checked={verified} onChange={event => setVerified(event.target.checked)} disabled={busy} /><span>我已通过可信渠道核对，确认此指纹属于目标服务器<div className="field-hint">信任仅在本进程有效。退出应用后需要重新核验。</div></span></label>
    {error && <p className="inline-error" role="alert">{error}</p>}
    <div className="modal-footer"><button className="button secondary" onClick={() => void decide(false)} disabled={busy}>拒绝并断开</button><button className={`button ${challenge.changed ? 'danger-button' : 'primary'}`} disabled={!verified || busy} onClick={() => void decide(true)}>{busy ? '正在处理…' : '信任并继续连接'}</button></div>
  </Modal>;
}
