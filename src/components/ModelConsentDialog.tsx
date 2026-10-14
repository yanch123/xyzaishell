import { useState } from 'react';
import { Modal } from './Modal';
import { Icon } from './Icon';
import type { ModelConfigStatus } from '../lib/model';

export function ModelConsentDialog({ config, changed, busy, sendAfter, error, onConfirm, onClose }: {
  config: ModelConfigStatus; changed: boolean; busy: boolean; sendAfter: boolean; error: string | null;
  onConfirm: () => Promise<void>; onClose: () => void;
}) {
  const [acknowledged, setAcknowledged] = useState(false);
  return <Modal title="确认模型接收方" eyebrow="EXPLICIT CHAT ONLY" onClose={onClose}>
    <p className="modal-intro">请核对下面的服务。确认后，此窗口中的聊天可向这个配置发送消息；配置重新加载后需要再次确认。</p>
    <dl className="model-details"><div><dt>请求地址</dt><dd>{config.destination}</dd></div><div><dt>模型</dt><dd>{config.model}</dd></div></dl>
    <div className="notice"><Icon name="info" /><span>每次发送包含：你在当前聊天输入的消息、该聊天已保留的历史，以及应用的聊天行为说明。API Key 仅由本机后端用于此服务的认证，不进入聊天正文或前端状态。服务可能按其条款记录内容或收费。</span></div>
    <p className="consent-scope">文字聊天模式不会自动读取或发送 SSH 终端内容、SSH 登录信息或配置文件，也不会执行命令。终端任务需要另行确认发送范围与就绪状态。请自行检查消息，避免发送密码、密钥或不希望交给该服务的数据。</p>
    <label className="check-field"><input type="checkbox" checked={acknowledged} disabled={busy || changed} onChange={event => setAcknowledged(event.target.checked)} /><span>我确认此地址和模型，并同意向此服务发送我主动发送的聊天及其历史。</span></label>
    {changed && <div className="inline-error" role="alert">配置已变化。请关闭此窗口，核对新接收方后重新确认。</div>}
    {error && <div className="inline-error" role="alert">{error}</div>}
    <div className="modal-footer"><button className="button secondary" onClick={onClose} disabled={busy}>暂不发送</button><button className="button primary" onClick={() => void onConfirm()} disabled={!acknowledged || busy || changed}>{busy ? '正在确认…' : sendAfter ? '同意并发送' : '同意此接收方'}</button></div>
  </Modal>;
}
