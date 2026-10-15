import { useEffect, useRef, useState } from 'react';
import type { HistoryStatus } from '../lib/storageBridge';
import type { ModelConfigStatus, ModelKeyUpdate, ModelTestKind, ModelTestResult } from '../lib/model';
import { HistorySettings } from './HistorySettings';
import { TerminalSettings } from './TerminalSettings';
import { NonModalPanel } from './NonModalPanel';
import { Icon } from './Icon';
import type { ModelChatController } from '../hooks/useModelChat';
import './SettingsDialog.css';

type PendingTest = { config: ModelConfigStatus; kind: ModelTestKind };
const TEST_LABELS: Record<ModelTestKind, string> = { chat_connectivity: '文字连通性', task_capability: '任务工具调用' };
const TASK_TEST_PROMPT = 'This is a synthetic compatibility test. Call xyz_capability_probe exactly once with the JSON argument {"probe":"xyz-task-capability"}. Do not call any other tool. The tool is inert and will not be executed.';
const TASK_TEST_TOOL = '{"name":"xyz_capability_probe","description":"Inert compatibility probe. The application validates the synthetic arguments only; it never executes this tool.","parameters":{"type":"object","additionalProperties":false,"required":["probe"],"properties":{"probe":{"type":"string","enum":["xyz-task-capability"]}}}}';
function testHeading(result: ModelTestResult): string {
  if (result.error?.code === 'MODEL_CANCELLED') return '测试已取消';
  if (result.outcome === 'unsupported') return result.kind === 'task_capability' ? '本次未通过工具调用格式检查' : '本次未通过文字连通性检查';
  if (result.outcome !== 'supported' || !result.success) return '测试结果不确定';
  return result.kind === 'task_capability' ? '合成工具调用格式已验证' : '流式聊天测试通过';
}
function testBoundary(kind: ModelTestKind): string {
  return kind === 'task_capability' ? '仅验证固定的合成工具调用格式，不执行工具，不连接 SSH。通过不代表真实任务一定可执行、安全或可靠；每次终端任务仍需单独授权与就绪检查。' : '仅检查流式文字连通性，不验证工具调用或任务执行。';
}

function normalizeDestination(value: string): string | null {
  try {
    const url = new URL(value);
    if (url.username || url.password || url.search || url.hash || !url.hostname || url.hostname.endsWith('.invalid')) return null;
    if (url.protocol !== 'https:' && !(url.protocol === 'http:' && ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname))) return null;
    const path = url.pathname.replace(/\/+$/, '');
    if (path.endsWith('/chat/completions') || path.endsWith('/responses')) return null;
    url.pathname = path;
    return url.toString().replace(/\/+$/, '');
  } catch { return null; }
}
export function SettingsDialog({ onClose, model, onConsent, onHistoryStatus }: {
  onClose: () => void; model: ModelChatController; onConsent: () => void;
  onTest?: () => Promise<void>; onHistoryStatus?: (status: HistoryStatus) => void;
}) {
  const { config } = model;
  const [category, setCategory] = useState<'model' | 'terminal' | 'general' | 'history'>('model');
  const [baseline, setBaseline] = useState(config);
  const [baseUrl, setBaseUrl] = useState(config.base_url ?? config.destination ?? '');
  const [modelName, setModelName] = useState(config.model ?? '');
  const [newKey, setNewKey] = useState('');
  const [keyMode, setKeyMode] = useState<'keep' | 'replace' | 'clear'>('keep');
  const [reuse, setReuse] = useState(false);
  const [clearDecision, setClearDecision] = useState(false);
  const [leaveDecision, setLeaveDecision] = useState<'close' | 'reload' | null>(null);
  const [testKind, setTestKind] = useState<ModelTestKind>('chat_connectivity');
  const [pendingTest, setPendingTest] = useState<PendingTest | null>(null);
  const [testConsent, setTestConsent] = useState(false);
  const [validation, setValidation] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const dirty = baseUrl !== (baseline.base_url ?? baseline.destination ?? '') || modelName !== (baseline.model ?? '') || keyMode !== 'keep';
  const latest = useRef({ config, dirty });
  latest.current = { config, dirty };
  const normalized = normalizeDestination(baseUrl);
  const changedTarget = !!baseline.key_present && normalized !== baseline.destination;
  const reuseRequired = changedTarget && keyMode === 'keep';
  const stale = dirty && baseline.file_revision !== config.file_revision;
  const invalidModel = !modelName.trim() || new TextEncoder().encode(modelName).length > 256 || /[\x00-\x1f\x7f]/.test(modelName) || modelName.includes('REPLACE_WITH');
  const invalidKey = keyMode === 'replace' && (!newKey || newKey.trim() !== newKey || new TextEncoder().encode(newKey).length > 16384 || /[\x00-\x1f\x7f]/.test(newKey) || newKey.includes('REPLACE_WITH'));
  const reset = (next: ModelConfigStatus) => {
    setBaseline(next); setBaseUrl(next.base_url ?? next.destination ?? ''); setModelName(next.model ?? '');
    setNewKey(''); setKeyMode('keep'); setReuse(false); setClearDecision(false); setValidation(false);
  };
  useEffect(() => {
    // Refresh a pristine editor, but never let a late event overwrite a draft.
    if (!dirty && config.file_revision !== baseline.file_revision) reset(config);
  }, [config, baseline.file_revision, dirty]);
  const performLeave = async (action: 'close' | 'reload') => {
    setLeaveDecision(null); setPendingTest(null); setMessage(null);
    if (action === 'close') onClose();
    else { reset(config); await model.reloadConfig(); }
  };
  const requestLeave = (action: 'close' | 'reload') => {
    if (model.configBusy) return;
    if (dirty) setLeaveDecision(action); else void performLeave(action);
  };
  const save = async (testAfter = false, leaveAfter: 'close' | 'reload' | null = null) => {
    const selectedKind = testKind;
    setValidation(true); setMessage(null); setPendingTest(null);
    if (!normalized || invalidModel || invalidKey || reuseRequired && !reuse || !baseline.file_revision || stale) return;
    const keyUpdate: ModelKeyUpdate = keyMode === 'replace' ? { kind: 'replace', value: newKey } : { kind: keyMode };
    const saved = await model.saveConfig({ expected_file_revision: baseline.file_revision, base_url: baseUrl, model: modelName, key_update: keyUpdate, reuse_key_for_destination: reuseRequired && reuse ? normalized : null });
    if (!saved) return;
    reset(saved); setLeaveDecision(null);
    setMessage(saved.ready ? '模型设置已保存。发送聊天前需确认接收方。' : '模型设置已保存，尚未配置 API Key，发送已禁用。');
    if (leaveAfter) { await performLeave(leaveAfter); return; }
    if (testAfter && saved.ready) { setPendingTest({ config: saved, kind: selectedKind }); setTestConsent(false); setMessage('模型设置已保存，测试待确认。'); }
  };
  const beginTest = async () => {
    if (model.configWriteBlocked || model.configBusy || dirty || !pendingTest || !testConsent || pendingTest.config.file_revision !== config.file_revision || pendingTest.kind !== testKind) return;
    const expected = pendingTest; setPendingTest(null); setMessage('设置已保存，正在测试已保存版本…');
    const result = await model.testSaved(expected.config, expected.kind);
    if (!result) setMessage('设置已保存，测试未完成。请核对错误；不会自动重试。');
    else if (result.error?.code === 'MODEL_CANCELLED') setMessage('设置已保存，测试已取消。');
    else if (latest.current.config.file_revision !== result.file_revision || latest.current.dirty) setMessage('此前保存版本的测试已结束，结果不代表当前草稿或新配置。');
    else if (result.outcome === 'supported' && result.success) setMessage(result.kind === 'task_capability' ? '设置已保存，合成工具调用格式已验证。真实任务仍需单独授权与就绪检查。' : '设置已保存，流式聊天测试通过。未验证工具调用。');
    else if (result.outcome === 'unsupported') setMessage('设置已保存，本次未通过所选测试的格式检查。不会自动重试。');
    else setMessage('设置已保存，测试结果不确定，不能据此判定模型不支持。不会自动重试。');
  };
  const testResultCurrent = model.configTest?.file_revision === config.file_revision;
  const canSave = model.native && !model.configBusy && !model.configWriteBlocked && !!baseline.file_revision && !stale;
  return <NonModalPanel title="工作空间设置" eyebrow="XYZAI SHELL · v0.1.3" onClose={() => requestLeave('close')} className="settings-modal">
    <nav className="settings-categories" aria-label="设置分类">
      {([['model', '模型'], ['terminal', '终端'], ['general', '通用'], ['history', '历史记录']] as const).map(([value, label]) => <button key={value} type="button" aria-current={category === value ? 'page' : undefined} onClick={() => setCategory(value)}>{label}</button>)}
    </nav>
    {category === 'model' && <div className="settings-model-editor">
      <div className="settings-model-heading"><h3>模型连接</h3><span className={dirty ? 'subtle-badge' : 'feature-badge'}>{!model.native ? '网页预览' : dirty ? '有未保存修改' : config.file_revision ? '已读取保存状态' : '需要重新读取'}</span></div>
      <p>OpenAI-compatible Chat Completions。保存只写入本机配置；测试只发送下方确认的固定合成数据。</p>
      <fieldset className="model-fields" disabled={!model.native || model.configBusy}>
        <label className="field">Base URL<input type="url" value={baseUrl} onChange={event => { setBaseUrl(event.target.value); setReuse(false); setPendingTest(null); setMessage(null); }} placeholder="https://api.example.com/v1" autoComplete="off" spellCheck={false} aria-invalid={validation && !normalized} /><span className="field-hint">填写 API 根地址，不包含 /chat/completions。HTTP 仅限在高级文件中明确允许的本机测试。</span>{validation && !normalized && <span className="field-error">请填写有效 HTTPS API 根地址，不含账号、查询参数或片段。</span>}</label>
        <label className="field">API Key<input type="password" value={newKey} onChange={event => { const value = event.target.value; setNewKey(value); setKeyMode(value ? 'replace' : 'keep'); setPendingTest(null); setMessage(null); }} placeholder={baseline.key_present ? '已配置，留空保持不变' : '输入新的 API Key'} autoComplete="new-password" spellCheck={false} disabled={keyMode === 'clear'} aria-invalid={validation && invalidKey} /><span className="field-hint">{keyMode === 'clear' ? '保存后将清除 Key，并禁用模型发送。' : baseline.key_present ? '已有 Key 只保留在本机后端，不会回填。' : 'Key 将以明文保存在受限的本机 model.toml 中。'}</span>{validation && invalidKey && <span className="field-error">请输入非占位 Key，不含首尾空格或控制字符。</span>}</label>
        <div className="settings-key-actions">{keyMode !== 'keep' && <button type="button" className="button secondary" onClick={() => { setNewKey(''); setKeyMode('keep'); setClearDecision(false); }}>取消{keyMode === 'clear' ? '清除' : '替换'}</button>}{baseline.key_present && keyMode !== 'clear' && <button type="button" className="button secondary" onClick={() => setClearDecision(true)}>清除已保存 Key…</button>}</div>
        {clearDecision && <div className="settings-decision" role="group" aria-label="确认清除 Key"><p>清除这个模型配置的 API Key？保存后不能再发送新模型请求，已启动请求仍使用原快照。</p><button type="button" className="button danger" onClick={() => { setKeyMode('clear'); setNewKey(''); setClearDecision(false); setPendingTest(null); }}>确认清除，保存时生效</button><button type="button" className="button secondary" onClick={() => setClearDecision(false)}>保留 Key</button></div>}
        <label className="field">模型名<input value={modelName} onChange={event => { setModelName(event.target.value); setPendingTest(null); setMessage(null); }} placeholder="精确模型标识" autoComplete="off" spellCheck={false} aria-invalid={validation && invalidModel} />{validation && invalidModel && <span className="field-error">请输入有效模型名，最多 256 字节。</span>}</label>
        {reuseRequired && <label className="check-field model-reuse"><input type="checkbox" checked={reuse} onChange={event => setReuse(event.target.checked)} /><span>将已有 Key 用于这个新目标：{normalized ?? '请先填写有效地址'}。这只允许保存，发送数据仍需另行确认。</span></label>}
        {validation && reuseRequired && !reuse && <div className="inline-error" role="alert">地址已变化。请输入新 Key、清除 Key，或明确选择将已有 Key 用于新目标。</div>}
      </fieldset>
      {stale && <div className="inline-error" role="alert">已保存配置发生变化。当前草稿未被覆盖；请重新读取后核对，不能覆盖新版本。</div>}
      {config.warning && <div className="notice warning" role="status"><Icon name="info" /><span>{config.warning}</span></div>}
      {config.error && <div className="inline-error" role="alert">{config.error.safe_message}</div>}
      {model.configWriteBlocked && <div className="inline-error" role="alert">保存结果需要核对。请先重新加载模型配置；不会重复保存或发起测试。</div>}
      {model.error && <div className="inline-error" role="alert">{model.error}</div>}
      <fieldset className="settings-test-kinds" disabled={model.configBusy || model.configTestBusy}>
        <legend>选择测试范围</legend>
        {(['chat_connectivity', 'task_capability'] as const).map(kind => <label className="settings-test-kind" key={kind}><input type="radio" name="model-test-kind" value={kind} checked={testKind === kind} onChange={() => { setTestKind(kind); setPendingTest(null); setTestConsent(false); setMessage(null); }} /><span><strong>{TEST_LABELS[kind]}</strong><span>{kind === 'chat_connectivity' ? '发送固定文字，检查能否接收流式回复。' : '发送固定工具声明，检查合成工具调用格式。'}</span></span></label>)}
        <p>{testBoundary(testKind)}</p>
      </fieldset>
      <div className="settings-actions">
        <button type="button" className="button primary" disabled={!canSave} onClick={() => void save()}>{model.configBusy ? '正在保存 / 读取…' : '保存'}</button>
        <button type="button" className="button secondary" disabled={!canSave || model.configTestBusy || keyMode === 'clear'} onClick={() => void save(true)}>保存并测试</button>
        {!dirty && config.ready && <button type="button" className="button secondary" disabled={!model.native || model.configBusy || model.configWriteBlocked || model.configTestBusy} onClick={() => { setPendingTest({ config, kind: testKind }); setTestConsent(false); setMessage(null); }}>测试已保存配置</button>}
        {!dirty && config.ready && !config.consented && <button type="button" className="button secondary" disabled={model.configBusy} onClick={onConsent}>确认聊天接收方</button>}
        {model.configTestBusy && <button type="button" className="button secondary" onClick={() => void model.cancelConfigTest()}>取消测试</button>}
      </div>
      {message && <p className="settings-save-status" role="status">{message}</p>}
      {model.configTest && <div className="settings-test-result" role="status" data-outcome={model.configTest.outcome ?? 'inconclusive'}><strong>{testHeading(model.configTest)}</strong><p>测试范围：{TEST_LABELS[model.configTest.kind] ?? '未知测试范围'}</p><p>{model.configTest.destination} · {model.configTest.model}</p><p>{testResultCurrent && !dirty ? '结果对应当前已保存配置。' : '结果仅对应此前已保存的版本，不代表当前草稿或新配置已通过。'}</p><p>{testBoundary(model.configTest.kind)}</p>{model.configTest.outcome === 'inconclusive' && <p>网络、认证、取消或响应未完整结束等问题均可能让结果不确定，不能据此判定模型不支持。</p>}{model.configTest.error && <p>{model.configTest.error.safe_message}</p>}</div>}
      {pendingTest && <div className="settings-decision" role="group" aria-label="确认测试接收方">
        <h4>确认测试接收方</h4><p>{pendingTest.config.destination}</p><p>模型：{pendingTest.config.model}</p><p>测试范围：{TEST_LABELS[pendingTest.kind]}</p>
        <p>将使用已保存的 API Key 向上述目标认证。不包含聊天历史、聊天草稿、终端内容或 SSH 登录信息，不连接 SSH，不执行任何工具或命令。服务可能记录请求或收费。</p>
        {pendingTest.kind === 'chat_connectivity' ? <p>仅发送固定消息“Reply with OK.”，请求流式回复，输出上限 16 tokens。</p> : <><p>发送固定合成消息、下方工具声明和固定参数，要求一次工具调用，输出上限 128 tokens。</p><details className="settings-test-payload"><summary>查看将发送的合成数据</summary><p className="selectable-path">{TASK_TEST_PROMPT}</p><p className="selectable-path">{TASK_TEST_TOOL}</p></details><p>{testBoundary(pendingTest.kind)}</p></>}
        <label className="check-field"><input type="checkbox" checked={testConsent} onChange={event => setTestConsent(event.target.checked)} /><span>同意向这个已保存目标发送上述{TEST_LABELS[pendingTest.kind]}测试数据。</span></label>
        {pendingTest.config.file_revision !== config.file_revision && <p role="alert">已保存版本已变化，请重新选择测试并核对接收方。</p>}
        <div className="settings-actions"><button className="button primary" disabled={!testConsent || dirty || model.configBusy || model.configWriteBlocked || model.configTestBusy || pendingTest.config.file_revision !== config.file_revision || pendingTest.kind !== testKind} onClick={() => void beginTest()}>确认并测试</button><button className="button secondary" onClick={() => { setPendingTest(null); setTestConsent(false); setMessage('设置已保存，测试已取消，未发送请求。'); }}>暂不测试</button></div>
      </div>}
      <details className="config-guide" open={!!config.error}>
        <summary>高级配置与安全说明</summary>
        <dl className="model-details"><div><dt>配置路径</dt><dd className="selectable-path">{config.path ?? (model.native ? '配置路径不可用' : '桌面应用中显示实际路径')}</dd></div><div><dt>路径来源</dt><dd>{config.path_overridden ? 'XYZAI_MODEL_CONFIG 环境变量覆盖；修改后需重启' : '操作系统默认配置目录'}</dd></div><div><dt>聊天接收方确认</dt><dd>{config.consented ? '当前配置已确认' : '发送聊天前需确认'}</dd></div></dl>
        <p>保存保留已有合法的 request 超时 / 输出限制、capabilities 和 transport 字段。未知 schema 或损坏文件不会被覆盖。当前运行中的模型请求继续使用启动时的配置快照。</p>
        <p>API Key 仍是本机 TOML 明文，不使用系统保险库。请勿放入 Git、同步盘或共享目录。Linux 私有目录应为 0700、文件为 0600；Windows 使用仅当前用户及系统管理员可访问的本机目录。</p>
        <button type="button" className="button secondary" disabled={!model.native || model.configBusy} onClick={() => requestLeave('reload')}>重新加载模型配置</button>
      </details>
    </div>}
    {category === 'terminal' && <TerminalSettings />}
    {category === 'general' && <><div className="settings-section"><div className="settings-icon"><Icon name="terminal" /></div><div><h3>SSH 终端 <span className="feature-badge">独立连接</span></h3><p>密码认证、本机私钥认证、主机指纹核验、交互终端和窗口尺寸同步。每个进程只连接一台服务器；另一目标可以在独立工作窗口打开。</p></div></div><div className="settings-section"><div className="settings-icon"><Icon name="shield" /></div><div><h3>能力与边界</h3><p>文字聊天与终端任务分开。启动终端任务需要数据授权及 Shell 就绪；普通命令在验证就绪后继续，危险或不确定操作保留明确决定。停止 AI 不等于远端程序已停止。历史恢复只供阅读，不恢复执行或批准。</p></div></div></>}
    {category === 'history' && <HistorySettings onStatus={onHistoryStatus} native={model.native} onRestore={chat => { model.restoreChat(chat); requestLeave('close'); }} />}
    {leaveDecision && <div className="settings-decision" role="group" aria-label="处理未保存修改"><p>模型设置有未保存修改。{leaveDecision === 'reload' ? '重新读取会丢弃草稿。' : '关闭会丢弃草稿。'}</p><div className="settings-actions"><button className="button primary" disabled={!canSave} onClick={() => void save(false, leaveDecision)}>保存后{leaveDecision === 'reload' ? '重新读取' : '关闭'}</button><button className="button secondary" onClick={() => void performLeave(leaveDecision)}>放弃修改并{leaveDecision === 'reload' ? '重新读取' : '关闭'}</button><button className="button secondary" onClick={() => setLeaveDecision(null)}>继续编辑</button></div></div>}
    <div className="modal-footer"><button type="button" className="button secondary" disabled={model.configBusy} onClick={() => requestLeave('close')}>关闭设置</button></div>
  </NonModalPanel>;
}
