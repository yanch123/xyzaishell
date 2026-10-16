import type { SessionSnapshot } from '../lib/types';
import { useState } from 'react';
import { Modal } from './Modal';
import { Icon } from './Icon';
import type { ModelConfigStatus } from '../lib/model';

export function ModelConsentDialog({ config, target, changed, busy, sendAfter, error, onConfirm, onClose }: {
  target?: SessionSnapshot; config: ModelConfigStatus; changed: boolean; busy: boolean; sendAfter: boolean; error: string | null;
  onConfirm: () => Promise<void>; onClose: () => void;
}) {
  const [acknowledged, setAcknowledged] = useState(false);
  return <Modal title="确认模型接收方" eyebrow="对话与当前目标范围" onClose={onClose}>
    <p className="modal-intro">请核对下面的服务。确认后，此窗口中的聊天可向这个配置发送消息；当前目标范围内可按需执行普通命令。配置或目标变化后需要再次确认。</p>
    <dl className="model-details"><div><dt>请求地址</dt><dd>{config.destination}</dd></div><div><dt>模型</dt><dd>{config.model}</dd></div></dl>
    <p>执行目标：{target?.state === 'connected' ? `${target.username}@${target.host}:${target.port ?? 22}` : '当前无 SSH 连接，仅对话；连接服务器后将单独确认目标范围'}</p>
    <div className="notice"><Icon name="info" /><span>每次发送包含：你在当前聊天输入的消息、该聊天已保留的历史，以及应用的行为说明。执行时还包括当前服务器身份、命令、任务事实与步骤、限量脱敏终端输出。API Key 仅由本机后端用于此服务的认证，不进入聊天正文或前端状态。服务可能按其条款记录内容或收费。</span></div>
    <p className="consent-scope">普通命令可在左侧同一可见终端自动执行。危险命令仍须逐条核对并批准，未知或无法可靠处理的操作会暂停。终端选区附件需在预览中单独审阅同意；确认范围后，请返回附件重新核对再发送。请勿发送密码、密钥或不希望交给该服务的数据。</p>
    <label className="check-field"><input type="checkbox" checked={acknowledged} disabled={busy || changed} onChange={event => setAcknowledged(event.target.checked)} /><span>我确认此地址、模型和当前目标范围，同意上述数据发送与普通命令自动执行；危险命令仍需单独批准。</span></label>
    {changed && <div className="inline-error" role="alert">配置已变化。请关闭此窗口，核对新接收方后重新确认。</div>}
    {error && <div className="inline-error" role="alert">{error}</div>}
    <div className="modal-footer"><button className="button secondary" onClick={onClose} disabled={busy}>暂不发送</button><button className="button primary" onClick={() => void onConfirm()} disabled={!acknowledged || busy || changed}>{busy ? '正在确认…' : sendAfter ? '同意并发送' : '同意此接收方'}</button></div>
  </Modal>;
}
