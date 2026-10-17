import { useEffect, useLayoutEffect, useId, useRef, useState } from 'react';
import { useContextUsage } from '../hooks/useContextUsage';
import type { ModelChatController } from '../hooks/useModelChat';
import type { ChatSnapshot } from '../lib/model';
import { activeCompaction, DISPLAY_CATEGORIES, DIALOGUE_PARTS, occupancy, tokenLabel, type CompactionMode, type ContextEstimate, type ProviderUsage } from '../lib/context';
import './context.css';
const LABELS = { system_instructions: '系统指令', tool_definitions: '工具定义', fixed_rules: '固定规则', history_summary: '历史摘要', current_conversation: '当前对话' };
const PART_LABELS = { user_messages: '用户消息', assistant_messages: 'AI 回复', tool_calls: '工具调用', tool_results: '工具／终端结果', other_inputs: '其他实际输入' };
const REASONS = { manual: '手动整理', token_pressure: '输入预算占用', hard_byte_limit: '请求超过本地传输字节上限', both: '输入预算占用及传输字节上限' };
function Usage({ label, usage }: { label: string; usage: ProviderUsage | null | undefined }) {
  return <section className="context-usage"><strong>{label}</strong>{usage ? <><p>供应商{usage.completeness === 'partial' ? '部分' : ''}实报 · 输入 {tokenLabel(usage.input_tokens)} / 输出 {tokenLabel(usage.output_tokens)} tokens</p><p>{usage.model} · {usage.received_at}</p><p>请求 {usage.request_id} · {usage.request_kind}</p>{usage.cached_input_tokens != null && <p>缓存输入 {tokenLabel(usage.cached_input_tokens)}（输入的子集）</p>}</> : <p>未知 · 未收到供应商用量</p>}</section>;
}
const tokenCount = (value: unknown): value is number => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
function composition(estimate: ContextEstimate | undefined) {
  const supported = estimate?.category_schema_version === 2;
  const categories = DISPLAY_CATEGORIES.map(category => ({ category, source: supported ? estimate.categories?.find(item => item.category === category) : undefined }));
  const protocol = supported ? estimate.protocol_overhead : undefined;
  const input = estimate?.estimated_input_tokens;
  const values = categories.map(item => item.source?.estimated_tokens);
  const content = estimate?.content_input_tokens;
  const known = supported && estimate.categories?.length === DISPLAY_CATEGORIES.length && values.every(tokenCount)
    && tokenCount(input) && tokenCount(content) && values.reduce<number>((sum, value) => sum + (value ?? 0), 0) === content
    && protocol?.method === 'protocol_structure_v1' && tokenCount(protocol.known_framing_tokens) && tokenCount(protocol.template_allowance_tokens)
    && tokenCount(protocol.estimated_tokens) && protocol.known_framing_tokens + protocol.template_allowance_tokens === protocol.estimated_tokens
    && content + protocol.estimated_tokens === input;
  return { supported, known, categories, protocol, input, content };
}
function TokenShare({ value, input }: { value: number | null | undefined; input: number | null | undefined }) {
  return <span>{value == null ? '未知' : value === 0 ? '0 · 本次未发送' : `≈${tokenLabel(value)}`}{value != null && input != null && input > 0 && <> · 占输入 {Number((value / input * 100).toFixed(1))}%</>}</span>;
}
function Composition({ estimate }: { estimate: ContextEstimate | undefined }) {
  const data = composition(estimate);
  return <section aria-label="输入类别构成（估算）">
    {!data.known && <p role="status">{data.supported ? '分类暂不可用；已知的总输入仍独立显示。' : '分类暂不可用；需要新版计量数据，正在刷新。'}</p>}
    {data.known && data.input! > 0 && <><div className="context-composition" aria-hidden="true">{data.categories.map(({ category, source }, index) => <span key={category} style={{ flex: source!.estimated_tokens!, background: `hsl(${210 + index * 24} 35% ${45 + index * 4}%)` }} />)}<span className="context-protocol-space" style={{ flex: data.protocol!.estimated_tokens! }} /></div><p className="context-composition-note">留白为协议开销，已计入总输入。</p></>}
    {data.categories.map(({ category, source }) => {
      const children = DIALOGUE_PARTS.map(part => ({ category: part, estimated_tokens: source?.children?.find(child => child.category === part)?.estimated_tokens }));
      const childKnown = data.known && source?.children?.length === DIALOGUE_PARTS.length && children.every(child => tokenCount(child.estimated_tokens)) && children.reduce((sum, child) => sum + (child.estimated_tokens ?? 0), 0) === source.estimated_tokens;
      const row = <><span>{LABELS[category]}</span><TokenShare value={data.known ? source?.estimated_tokens : null} input={data.input} /></>;
      return category === 'current_conversation' ? <details className="context-dialogue" data-context-category={category} data-estimated-tokens={data.known ? source?.estimated_tokens : undefined} key={category}><summary>{row}</summary><div className="context-dialogue-parts">{!childKnown && <p>对话明细暂不可用</p>}{children.map(child => <div className="context-row" data-context-dialogue-part={child.category} data-estimated-tokens={childKnown ? child.estimated_tokens : undefined} key={child.category}><span>{PART_LABELS[child.category]}</span><TokenShare value={childKnown ? child.estimated_tokens : null} input={data.input} /></div>)}</div></details> : <div className="context-row" data-context-category={category} data-estimated-tokens={data.known ? source?.estimated_tokens : undefined} key={category}>{row}</div>;
    })}
  </section>;
}
export function ContextIndicator({ chat, draft, model, readOnly, onSettings }: { chat: ChatSnapshot; draft: string; model: ModelChatController; readOnly: boolean; onSettings: () => void }) {
  const context = useContextUsage(chat.chat_id, chat.revision, model.config.revision, draft, model.native && model.ready && !!model.config.revision && !readOnly);
  const [open, setOpen] = useState(false);
  const [mode, setMode] = useState<CompactionMode>('llm');
  const [saving, setSaving] = useState(false);
  const [position, setPosition] = useState({ left: 12, bottom: 100, maxHeight: 500 });
  const saveLock = useRef(false);
  const button = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const host = useRef<HTMLDivElement>(null);
  const id = useId();
  const { snapshot, job } = context;
  const budget = snapshot?.budget;
  const active = activeCompaction(job);
  const failed = job?.state === 'failed' || !!context.operationError;
  const ratio = occupancy(snapshot);
  const caption = active ? job?.state === 'waiting_safe_point' ? '等待压缩…' : '正在压缩…' : failed ? '压缩未完成' : `上下文 ${ratio}`;
  const close = () => { setOpen(false); button.current?.focus(); };
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const rect = button.current?.getBoundingClientRect();
      if (!rect) return;
      setPosition({ left: Math.max(12, Math.min(rect.right - 330, window.innerWidth - 342)), bottom: window.innerHeight - rect.top + 10, maxHeight: Math.max(120, rect.top - 22) });
    };
    place(); window.addEventListener('resize', place);
    return () => window.removeEventListener('resize', place);
  }, [open]);
  useEffect(() => {
    if (!open) return;
    panel.current?.focus();
    const outside = (event: PointerEvent) => { if (!host.current?.contains(event.target as Node)) setOpen(false); };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [open]);
  const updateAuto = async (enabled: boolean, nextMode: CompactionMode) => {
    const config = model.config;
    if (saveLock.current || !config.file_revision || !config.base_url || !config.model) return;
    saveLock.current = true; setSaving(true);
    try { await model.saveConfig({ expected_file_revision: config.file_revision, base_url: config.base_url, model: config.model, key_update: { kind: 'keep' }, reuse_key_for_destination: null, context_auto_compact: enabled, context_auto_mode: nextMode }); }
    finally { saveLock.current = false; setSaving(false); }
  };
  const auto = model.config.context_auto_compact ?? true;
  const autoMode = model.config.context_auto_mode ?? 'llm';
  const parts = composition(snapshot?.estimate);
  const available = context.fresh && !!snapshot && snapshot.status === 'ready' && ratio !== '—';
  return <div className="context-control" ref={host}>
    <button ref={button} type="button" className="context-indicator" aria-expanded={open} aria-controls={id} aria-label={`可用输入预算占用 ${ratio}，${caption}，查看上下文`} title="可用输入预算占用（估算）" onClick={() => setOpen(value => !value)}>{caption}</button>
    {open && <div ref={panel} tabIndex={-1} id={id} className="context-popover" style={position} role="dialog" aria-modal="false" aria-label="上下文占用与压缩" data-compaction-job-id={job?.job_id} data-compaction-state={job?.state} data-compaction-mode={job?.mode} data-category-schema-version={snapshot?.estimate.category_schema_version} data-content-input-tokens={parts.known ? parts.content : undefined} data-protocol-overhead-tokens={parts.known ? parts.protocol?.estimated_tokens : undefined} data-estimated-input-tokens={parts.input} onKeyDown={event => { if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(); } }}>
      <div className="context-heading"><strong>下一次请求</strong><button type="button" aria-label="关闭上下文" onClick={close}>×</button></div>
      <p className="context-total">输入约 {tokenLabel(budget?.decision_input_tokens)} tokens <span>· {ratio}</span></p>
      <p>{budget ? `窗口 ${tokenLabel(budget.context_window_tokens)} · ${{ product_default: '按默认容量估算', user_override: '手动设置', verified_metadata: '已验证模型元数据', unavailable: '容量配置不可用' }[budget.capacity_source]}` : '暂无法估算'}</p>
      {snapshot?.estimate.method === 'heuristic' && <p>保守估算 · 并非供应商实报</p>}{budget && (!budget.input_budget_tokens || budget.input_budget_tokens < 0) && <p role="alert">容量配置无效，自动压缩已暂停；请设置模型容量。</p>}
      {snapshot?.measurement_scope === 'draft_preview' && <p>含未发送草稿 · 仅在本机预估{context.committed ? `；已提交上下文 ${occupancy(context.committed)}` : ''}</p>}
      {snapshot && (!context.fresh || snapshot.status !== 'ready') && <p role="status">{snapshot.status === 'unavailable' ? '暂无法估算，请检查模型容量配置' : '正在更新估算…'}</p>}
      {!model.native && <p>网页预览 · 原生上下文计量不可用</p>}{readOnly && <p>历史仅供阅读，不能压缩为可执行上下文</p>}
      <Composition estimate={snapshot?.estimate} />
      <div className="context-reserves"><div className="context-row"><span>输出预留</span><span>{tokenLabel(budget?.output_reserve_tokens)}</span></div><div className="context-row"><span>安全余量</span><span>{tokenLabel(budget?.safety_margin_tokens)}</span></div></div>
      <div className="context-row"><label><input type="checkbox" checked={auto} disabled={!model.native || readOnly || saving || model.configBusy || model.configWriteBlocked} onChange={event => void updateAuto(event.target.checked, autoMode)} /> 自动压缩</label><select aria-label="自动压缩方式" value={autoMode} disabled={!model.native || readOnly || saving || model.configBusy || model.configWriteBlocked} onChange={event => void updateAuto(auto, event.target.value as CompactionMode)}><option value="llm">LLM</option><option value="quick">快速</option></select></div>
      <p>回复后 80%，任务中 95%{budget && ` · 当前${snapshot?.turn_phase === 'end_of_turn' ? '回复后' : '任务中'} ${Math.round(budget.effective_trigger_ratio * 100)}%`}</p>
      <div className="context-actions"><select aria-label="本次压缩方式" value={mode} disabled={active || context.pending} onChange={event => setMode(event.target.value as CompactionMode)}><option value="llm">大模型压缩</option><option value="quick">快速压缩</option></select><button type="button" className="button secondary" disabled={!available || readOnly || context.pending || context.uncertain || active || mode === 'llm' && (!model.config.consented || !model.config.ready)} onClick={() => void context.compact(mode)}>{context.pending ? '正在核对…' : failed ? '重试压缩' : '立即压缩'}</button></div>
      <p>{mode === 'llm' ? '使用当前已授权模型整理旧消息，可能遗漏细节，会产生额外模型请求。原文可用性以历史保存状态为准。' : '本地整理，不调用模型，保留用户输入及现用摘要，其他旧内容可能丢失细节。'}仅整理模型上下文。</p>
      {active && <div role="status"><p>{job?.state === 'waiting_safe_point' ? '将在安全边界压缩；不会打断生成、执行或审批。' : '正在整理已提交上下文…'}</p><button type="button" disabled={context.pending} onClick={() => void context.cancel()}>取消压缩</button></div>}
      {job?.state === 'succeeded' && !context.operationError && !context.pending && <p role="status">上下文已整理</p>}{job?.state === 'no_effect' && <p role="status">未能进一步缩减</p>}{job?.state === 'cancelled' && <p role="status">压缩已取消，原上下文保留</p>}{job?.state === 'stale' && <p role="status">上下文已变化，本次结果未采用</p>}
      {job?.state === 'failed' && job.error && <p role="alert">{job.error.safe_message}</p>}{context.error && <p role="alert">{context.error}</p>}{context.uncertain && <p>操作结果尚未确认，已阻止重复提交。<button onClick={() => void context.refresh()}>核对状态</button></p>}
      <details><summary>详情</summary>
        <p>可用输入预算 {tokenLabel(budget?.input_budget_tokens)} tokens（窗口减输出预留及安全余量）。</p>
        {budget?.capacity_source === 'product_default' && <p>256K = 256000 tokens，是产品默认配置；服务商实际容量未验证。</p>}
        <button type="button" onClick={() => { close(); onSettings(); }}>设置模型容量</button>
        <section aria-label="输入估算对账"><strong>协议开销（估算，已计入总输入）</strong>
          {parts.known ? <><p>五类内容 {parts.content} + 协议开销 {parts.protocol!.estimated_tokens} = 总输入 {parts.input} tokens</p><p>协议开销占输入 {parts.input! > 0 ? `${Number((parts.protocol!.estimated_tokens! / parts.input! * 100).toFixed(1))}%` : '—（无输入）'}</p><p>已知包装 {parts.protocol!.known_framing_tokens}，模板工程余量 {parts.protocol!.template_allowance_tokens} tokens；并非供应商实测。</p></> : <p>分类或协议开销未知，暂无法对账；总输入估算 {tokenLabel(parts.input)} tokens。</p>}
        </section>
        {budget?.decision_input_tokens != null && parts.input != null && parts.input !== budget.decision_input_tokens && <p>分类对应总输入估算 {parts.input} tokens；占用和自动阈值采用保守值 {budget.decision_input_tokens} tokens，差额 {budget.decision_input_tokens - parts.input} tokens。</p>}
        <p>计量方法：{snapshot?.estimate.estimator_version ?? '未知'}。五类及协议份额以总输入估算为分母，与预算占用不同。</p>
        {snapshot?.byte_guards.filter(guard => guard.exceeded).map(guard => <p key={guard.scope} role="alert">请求超过本地大小限制：{guard.measured_bytes} / {guard.limit_bytes} 字节</p>)}
        <Usage label="上一次主请求用量" usage={snapshot?.last_request_usage} /><Usage label="最近压缩请求用量" usage={snapshot?.last_compaction_usage} />
        {job && <section><strong>最近压缩</strong><p>{job.mode === 'llm' ? '大模型' : '快速'} · {REASONS[job.reason]} · {job.state}</p>{job.before_input_tokens != null && job.after_input_tokens != null && <p>输入估算约 {tokenLabel(job.before_input_tokens)} → {tokenLabel(job.after_input_tokens)} tokens</p>}{job.before_request_bytes != null && job.after_request_bytes != null && <p>请求大小 {job.before_request_bytes} → {job.after_request_bytes} 字节</p>}{job.error && <p role="alert">{job.error.safe_message}</p>}</section>}
        <p>压缩不删除已保存的对话，不发送草稿，不续跑终端命令。</p>
      </details>
    </div>}
  </div>;
}
