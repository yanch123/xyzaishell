import type { SessionSnapshot } from '../lib/types';
import { useState } from 'react';
import { Modal } from './Modal';
import type { ModelConfigStatus } from '../lib/model';

export function ModelConsentDialog({ config, target, changed, busy, sendAfter, error, onConfirm, onClose }: {
  target?: SessionSnapshot; config: ModelConfigStatus; changed: boolean; busy: boolean; sendAfter: boolean; error: string | null;
  onConfirm: () => Promise<void>; onClose: () => void;
}) {
  const [acknowledged, setAcknowledged] = useState(false);
  return <Modal title="确认模型接收方" eyebrow="对话与当前目标范围" onClose={onClose}>
    <p className="modal-intro">首次使用，请确认接收方和当前服务器。之后在此范围内无需每次确认。</p>
    <dl className="model-details"><div><dt>接收方</dt><dd>{config.destination}</dd></div><div><dt>模型</dt><dd>{config.model}</dd></div><div><dt>服务器</dt><dd>{target?.state === 'connected' ? `${target.username}@${target.host}:${target.port ?? 22}` : '未连接，仅聊天'}</dd></div></dl>
    <ul className="consent-summary">
      <li>发送聊天、该聊天已保留的历史；执行时还发送服务器身份、命令、任务摘要和有限脱敏输出。</li>
      <li>普通命令在左侧终端自动执行；危险命令仍逐条询问。</li>
      <li>服务可能记录内容或收费。请勿在聊天中输入密码或密钥。</li>
    </ul>
    <details><summary>更多说明</summary><p>配置或目标范围变化后需重新确认。API Key 仅用于服务认证，不进入聊天。选区附件仍需单独预览、编辑和确认；脱敏不保证识别所有秘密。</p></details>
    <label className="check-field"><input type="checkbox" checked={acknowledged} disabled={busy || changed} onChange={event => setAcknowledged(event.target.checked)} /><span>我确认此地址、模型和当前目标范围，同意上述数据发送与普通命令自动执行；危险命令仍需单独批准。</span></label>
    {changed && <div className="inline-error" role="alert">配置已变化。请关闭此窗口，核对新接收方后重新确认。</div>}
    {error && <div className="inline-error" role="alert">{error}</div>}
    <div className="modal-footer"><button className="button secondary" onClick={onClose} disabled={busy}>暂不发送</button><button className="button primary" onClick={() => void onConfirm()} disabled={!acknowledged || busy || changed}>{busy ? '正在确认…' : sendAfter ? '同意并发送' : '同意此接收方'}</button></div>
  </Modal>;
}
