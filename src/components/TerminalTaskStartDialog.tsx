import { useState } from 'react';
import { Modal } from './Modal';
import { Icon } from './Icon';
import { visibleCommand } from '../lib/agent';
import type { ModelConfigStatus } from '../lib/model';
import type { SessionSnapshot } from '../lib/types';

export function TerminalTaskStartDialog({ config, target, instruction, changed, busy, error, integrationReady = false, onConfirm, onClose }: {
  config: ModelConfigStatus; target: SessionSnapshot; instruction: string; changed: boolean; busy: boolean;
  error: string | null; integrationReady?: boolean; onConfirm: () => Promise<void>; onClose: () => void;
}) {
  const [consent, setConsent] = useState(false);
  const [ready, setReady] = useState(false);
  return <Modal title="确认终端任务" eyebrow="CURRENT TASK · EXPLICIT CONSENT" onClose={() => { if (!busy) onClose(); }}>
    <p className="modal-intro">这次会真实操作当前 SSH 终端。请先核对接收方、服务器与任务，并查看左侧终端。</p>
    <dl className="model-details"><div><dt>模型接收方</dt><dd>{config.destination}</dd></div><div><dt>模型</dt><dd>{config.model}</dd></div><div><dt>SSH 目标</dt><dd dir="ltr">{visibleCommand(target.username ?? '')}@{visibleCommand(target.host ?? '')}:{target.port}</dd></div><div><dt>任务</dt><dd>{instruction}</dd></div></dl>
    <div className="notice warning"><Icon name="info" /><span>发送范围：本次任务指令、模型的任务分析、拟执行的命令，以及本次运行开始后收集的、有长度上限的终端输出。不会自动发送启动前的整段终端历史。服务可能记录内容或收费。输出可能含有敏感数据，过滤不能保证识别所有秘密；不要在聊天中输入密码或密钥。</span></div>
    <label className="check-field"><input type="checkbox" checked={consent} disabled={busy || changed} onChange={event => setConsent(event.target.checked)} /><span>我同意将本次任务上述数据发送到此地址。此前的文字聊天确认不替代这次授权。</span></label>
    {!integrationReady && <label className="check-field"><input type="checkbox" checked={ready} disabled={busy || changed} onChange={event => setReady(event.target.checked)} /><span>我已检查终端，确认它位于空的 Shell 提示符，可以接收命令；不是密码提示或其他前台程序。我会自行避免手动输入与 AI 输入冲突。</span></label>}
    {integrationReady && <p className="consent-scope">命令状态识别已确认当前终端可接收命令。执行期间请避免手动输入与 AI 输入冲突。</p>}
    <p className="consent-scope">每条命令先显示再派发；风险或不确定命令需要单次批准。停止 AI 不会发送 Ctrl+C，已写入的字符不能撤回。不同聊天共享同一终端，一次只有一个 AI 任务拥有写入权。</p>
    {changed && <div className="inline-error" role="alert">模型配置、SSH 连接或终端就绪状态已变化。请关闭窗口，核对新的目标后重新开始。</div>}
    {error && <div className="inline-error" role="alert">{error}</div>}
    <div className="modal-footer"><button className="button secondary" onClick={onClose} disabled={busy}>暂不执行</button><button className="button primary" disabled={!consent || (!ready && !integrationReady) || busy || changed} onClick={() => void onConfirm()}>{busy ? '正在启动…' : '同意并开始本次任务'}</button></div>
  </Modal>;
}
