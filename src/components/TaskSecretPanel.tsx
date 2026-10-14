import { useEffect, useRef, useState } from 'react';
import type { TerminalTaskSnapshot } from '../lib/agent';
import type { TerminalAgentController } from '../hooks/useTerminalAgent';
import { taskSecretsBridge } from '../lib/storageBridge';
import type { TaskSecretsView } from '../lib/storageBridge';
import { safeError } from '../lib/types';
export function TaskSecretPanel({ task, agent, connected }: { task: TerminalTaskSnapshot; agent: TerminalAgentController; connected: boolean }) {
  const [view, setView] = useState<TaskSecretsView | null>(null);
  const [field, setField] = useState(''); const [newField, setNewField] = useState('');
  const [purpose, setPurpose] = useState(task.secret_request?.purpose ?? '');
  const [confirmed, setConfirmed] = useState(false); const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null); const lock = useRef(false);
  const usable = agent.native && connected && task.owner && ['needs_user', 'needs_secrets'].includes(task.phase) && !agent.pending[task.task_id] && !agent.uncertain[task.task_id];
  useEffect(() => { let live = true; if (usable) void taskSecretsBridge.get(task.task_id, task.run_id).then(next => { if (live) setView(next); }).catch(e => { if (live) setError(safeError(e)); }); return () => { live = false; }; }, [task.task_id, task.run_id, usable]);
  useEffect(() => { setConfirmed(false); }, [task.revision, view?.snapshot_id, field, purpose, connected]);
  const operate = async (call: () => Promise<TaskSecretsView>) => {
    if (lock.current || !usable) return; lock.current = true; setBusy(true); setError(null); setConfirmed(false);
    try { setView(await call()); } catch (e) { setError(safeError(e)); } finally { lock.current = false; setBusy(false); }
  };
  const fields = task.secret_request?.fields ?? (newField.trim() ? [{ name: newField.trim(), required: true }] : []);
  return <details className="task-secret-panel" open={task.phase === 'needs_secrets'}><summary>任务专用秘密文件 · 不显示秘密值</summary>
    <p>任务 ID：<span className="selectable-path">{task.task_id}</span></p><p>仅在本机编辑器中填写此任务的秘密文件，不要粘贴到聊天。界面和模型只接收字段名称与是否已填写。</p>
    {view?.local_edit_path && <p>本机编辑路径：<span className="selectable-path">{view.local_edit_path}</span></p>}
    {task.secret_request && <p>所需字段：{task.secret_request.fields.map(item => `${item.name}${item.required ? '（必填）' : ''}`).join('、')}</p>}
    {!task.secret_request && <label>新增字段名称<input aria-label="新增秘密字段名称" value={newField} maxLength={64} onChange={e => setNewField(e.target.value)} placeholder="如 service_password（不要填写秘密值）" disabled={!usable || busy} /></label>}
    <div className="settings-actions"><button className="button secondary" disabled={!usable || busy || !fields.length} onClick={() => void operate(() => taskSecretsBridge.ensure(task.task_id, task.run_id, fields, view?.snapshot_id ?? null))}>准备本机秘密文件</button><button className="button secondary" disabled={!usable || busy} onClick={() => void operate(() => taskSecretsBridge.reload(task.task_id, task.run_id))}>重新加载已填写状态</button></div>
    {view && <><ul>{view.fields.map(item => <li key={item.name}>{item.name}：{item.present ? '已填写' : '未填写'}</li>)}</ul><label>本次输入字段<select aria-label="本次输入秘密字段" value={field} disabled={!usable || busy} onChange={e => setField(e.target.value)}><option value="">请选择已填写字段</option>{view.fields.filter(item => item.present).map(item => <option key={item.name} value={item.name}>{item.name}</option>)}</select></label></>}
    <label>本次输入用途<input aria-label="秘密输入用途" value={purpose} maxLength={512} onChange={e => setPurpose(e.target.value)} placeholder="例如目标服务的密码提示（不要填写密码）" disabled={!usable || busy} /></label>
    <p className="action-warning">只可输入到已核实的密码提示，不可用于 Shell 命令替换。将向 {task.target.username}@{task.target.host}:{task.target.port} 的当前 PTY 发送字段值及 Enter。远端程序可能回显、记录或转发秘密；无法确认时请手动处理。</p>
    <label className="check-field"><input type="checkbox" checked={confirmed} disabled={!usable || busy} onChange={e => setConfirmed(e.target.checked)} /><span>我已核对目标和用途，当前终端正等待此秘密输入，并理解远端回显风险。</span></label>
    <button className="button secondary" disabled={!usable || busy || !confirmed || !field || !purpose.trim()} onClick={() => { if (!confirmed) return; setConfirmed(false); void agent.prepareSecret(task.task_id, task.run_id, field, purpose); }}>核对本次秘密输入动作</button>
    <p className="field-hint">下一步仍需在动作卡片中逐次批准。不会向模型发送字段值，恢复历史也不会重放输入。</p>
    {error && <p className="inline-error" role="alert">{error}</p>}
  </details>;
}
