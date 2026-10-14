import { useEffect, useState } from 'react';
import type { ShellIntegrationController } from '../hooks/useShellIntegration';
import type { IntegrationPhase } from '../lib/integrationBridge';
import type { SessionSnapshot } from '../lib/types';
import { NonModalPanel } from './NonModalPanel';
export const INTEGRATION_LABELS: Record<IntegrationPhase, string> = { unintegrated: '尚未启用', installing: '正在启用', awaiting_ready: '等待命令提示符', ready: '命令识别已就绪', awaiting_begin: '等待命令开始', running: '前台命令运行中', unknown: '状态未知，需核对', unsupported: '当前终端环境不受支持', disconnected: '命令识别已断开' };
export function IntegrationPanel({ integration, snapshot, onClose }: { integration: ShellIntegrationController; snapshot: SessionSnapshot; onClose: () => void }) {
  const [consent, setConsent] = useState(false);
  useEffect(() => setConsent(false), [snapshot.session?.session_id, snapshot.session?.generation]);
  const phase = integration.status?.phase;
  return <NonModalPanel title="命令状态识别" eyebrow="仅在本次连接生效 · 手动终端始终可用" onClose={onClose} className="help-panel">
    <p>目标：{snapshot.username}@{snapshot.host}:{snapshot.port ?? 22}</p>
    <p role="status">{phase ? INTEGRATION_LABELS[phase] : snapshot.state === 'connected' ? '正在读取' : '请先连接 SSH'}</p>
    <p>启用后，AI 可以识别命令何时开始、何时返回提示符，再继续下一步。启用前，请确认已经完成登录、当前没有命令运行，并且看到 Bash 命令提示符。</p>
    <p>当前支持交互 Bash 5.1 / 5.2。启用会在当前终端执行临时识别脚本，不安装软件、不新建子 Shell，也不会自动保存操作日志。</p><details><summary>支持范围与识别原理</summary><p>支持 Bash 5.1 / 5.2 的常见默认配置，包括 Ubuntu 提示符、HISTCONTROL=ignoreboth 和关闭 lithist。需要开启 history、cmdhist、promptvars、interactive_comments，HISTIGNORE 为空且 histchars 保持默认 !^#。只允许显式 EXIT trap；其他 trap、提示符命令替换或不安全展开，以及 functrace / extdebug / errexit 等配置不受支持。现有 PS0 和 PROMPT_COMMAND 会保留并核对。发现不支持的配置时，手动终端仍可用。</p><p>前台返回不代表后台任务或命令的业务目标已经完成；远端输出和提示符文本不能批准命令。集成发生冲突或事件缺失时，先核对终端，不能用关闭提示来恢复自动执行。</p></details>
    {integration.error && <p className="inline-error" role="alert">{integration.error}</p>}
    {integration.status?.error && <p className="inline-error" role="alert">{integration.status.error.safe_message}</p>}
    {integration.uncertain && <p className="notice warning">启用结果需要核对，已阻止重复提交。请查看终端和当前状态；不会自动重发脚本。</p>}
    {(!phase || phase === 'unintegrated') && <><label className="check-field"><input type="checkbox" checked={consent} disabled={snapshot.state !== 'connected' || integration.busy || integration.uncertain} onChange={event => setConsent(event.target.checked)} /><span>当前终端已完成登录，已显示命令提示符，当前没有命令运行，可以正常输入命令。</span></label><button className="button primary" disabled={!consent || integration.busy || integration.uncertain || snapshot.state !== 'connected'} onClick={() => { setConsent(false); void integration.install(); }}>{integration.busy ? '正在启用…' : '启用命令识别'}</button></>}
    <div className="modal-footer"><button className="button secondary" onClick={onClose}>关闭</button></div>
  </NonModalPanel>;
}
