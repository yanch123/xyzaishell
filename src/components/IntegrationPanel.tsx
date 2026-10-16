import type { ShellIntegrationController } from '../hooks/useShellIntegration';
import type { IntegrationPhase } from '../lib/integrationBridge';
import { sameSession } from '../lib/types';
import type { SessionSnapshot } from '../lib/types';
import { NonModalPanel } from './NonModalPanel';
export const INTEGRATION_LABELS: Record<IntegrationPhase, string> = { unintegrated: '仅手动 · 尚无 Shell 就绪证据', installing: '自动准备中', awaiting_ready: '自动准备 · 等待 Shell 就绪', ready: '可执行 · Shell 已就绪', awaiting_begin: '等待命令开始', running: '前台命令运行中', unknown: '仅手动 · 状态未知，需核对', unsupported: '仅手动 · 当前环境不受支持', disconnected: '命令识别已断开' };
export function IntegrationPanel({ integration, snapshot, onClose }: { integration: ShellIntegrationController; snapshot: SessionSnapshot; onClose: () => void }) {
  const phase = integration.status?.phase;
  return <NonModalPanel title="命令状态识别" eyebrow="仅在本次连接生效 · 手动终端始终可用" onClose={onClose} className="help-panel">
    <p>目标：{snapshot.username}@{snapshot.host}:{snapshot.port ?? 22}</p>
    <p role="status">{phase ? INTEGRATION_LABELS[phase] : snapshot.state === 'connected' ? '正在读取' : '请先连接 SSH'}</p>
    <p>连接后自动准备。不能自动执行时，仍可聊天和手动使用终端。</p>
    <details><summary>支持范围与识别原理</summary>{snapshot.state === 'connected' && sameSession(integration.status?.session, snapshot.session) && integration.status?.manual_tracking === 'prompt_gap_only' && <p aria-label="手动命令记录能力">当前 Shell 的手动命令仅能记录观察缺口；AI 执行命令仍可识别开始、前台返回和后续就绪。此限制不代表 Shell 忙碌或日志已启用。</p>}<p>连接后会在当前终端自动启动 Bash 4.2 / 5.1 / 5.2 并读取 ~/.bashrc，临时准备命令执行。使用非登录 Shell，不读取登录配置文件；不修改远程配置，也不自动保存日志。不兼容或启动需要交互时，仍可使用手动终端。</p><p>支持 Bash 4.2 / 5.1 / 5.2 的常见默认配置，包括 Ubuntu 提示符、HISTCONTROL=ignoreboth 和关闭 lithist。需要开启 history、cmdhist、promptvars、interactive_comments，HISTIGNORE 为空且 histchars 保持默认 !^#。只允许显式 EXIT trap；其他 trap、提示符命令替换或不安全展开，以及 functrace / extdebug / errexit 等配置不受支持。现有 PS0 和 PROMPT_COMMAND 会保留并核对。发现不支持的配置时，手动终端仍可用。</p><p>前台返回不代表后台任务或命令的业务目标已经完成；远端输出和提示符文本不能批准命令。集成发生冲突或事件缺失时，先核对终端，不能用关闭提示来恢复自动执行。</p></details>
    {integration.error && <p className="inline-error" role="alert">{integration.error}</p>}
    {integration.status?.error && <p className="inline-error" role="alert">{integration.status.error.safe_message}</p>}
    {integration.uncertain && <p className="notice warning">启用结果需要核对，已阻止重复提交。请查看终端和当前状态；不会自动重发脚本。</p>}
    <div className="modal-footer"><button className="button secondary" onClick={onClose}>关闭</button></div>
  </NonModalPanel>;
}
