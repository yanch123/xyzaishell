import type { TaskContext, TerminalTaskSnapshot } from '../lib/agent';
import { sameSession } from '../lib/types';
import './task-context.css';

const STEP_LABELS = { pending: '待处理', in_progress: '进行中', blocked: '受阻', completed: '已完成（模型标记）' } as const;
function EvidenceReferences({ ids, context }: { ids: string[]; context: TaskContext }) {
  return <div className="context-evidence-list">{ids.map(id => {
    const evidence = context.evidence.find(item => item.command_id === id);
    return evidence ? <details key={id}><summary>关联命令：{evidence.command_text}</summary>
      <p>退出码：{evidence.exit_code ?? '未知'} · {evidence.command_completed ? '前台已返回' : '完成状态未确认'}</p>
      <p className="context-caveat">该轮观察片段可能混有手动操作或后台输出，不保证全部来自这条命令。</p>
      <pre>{evidence.output_excerpt || '没有保留输出片段'}</pre>
      {(evidence.output_truncated || evidence.output_gap) && <p className="notice warning">输出不完整，不能据此认定未显示的内容不存在。</p>}
    </details> : <span key={id}>来源片段已不在当前保留范围内</span>;
  })}</div>;
}
export function TaskContextPanel({ task }: { task: TerminalTaskSnapshot }) {
  const context = task.task_context;
  if (!context || context.task_id !== task.task_id || context.run_id !== task.run_id || !sameSession(context.session, task.session)) return null;
  return <details className="task-context-panel">
    <summary>已确认事实 / 剩余步骤 <span>{context.findings.length} 条结论 · {context.remaining_steps.filter(step => step.status !== 'completed').length} 项待处理</span></summary>
    <p className="context-scope">仅属于本次任务 · {task.target.username}@{task.target.host}:{task.target.port}</p>
    {context.summary && <p>{context.summary}</p>}
    <section aria-label="已确认事实"><h4>已确认事实</h4>
      <p className="context-caveat">以下结论根据命令输出由模型整理，尚非独立核验；终端内容仍可能不完整或不可信。</p>
      {context.findings.length ? <ul>{context.findings.map(fact => <li key={fact.id}><p>{fact.text}</p><EvidenceReferences ids={fact.evidence_command_ids} context={context} /></li>)}</ul> : <p>尚无有来源依据的排查结论。</p>}
      {context.confirmed_facts.length > 0 && <details><summary>程序识别的命令返回记录（{context.confirmed_facts.length}）</summary><ul>{context.confirmed_facts.map(fact => <li key={fact.command_id}>命令 {fact.command_id} 前台已返回，退出码 {fact.exit_code}。不代表业务目标完成。</li>)}</ul></details>}
    </section>
    {context.hypotheses.length > 0 && <section aria-label="待验证推测"><h4>待验证推测</h4><ul>{context.hypotheses.map(item => <li key={item.id}><p>{item.text}</p><EvidenceReferences ids={item.evidence_command_ids} context={context} /></li>)}</ul></section>}
    <section aria-label="剩余步骤"><h4>剩余步骤</h4>{context.remaining_steps.length ? <ol>{context.remaining_steps.map(step => <li key={step.id}><span className="subtle-badge">{STEP_LABELS[step.status]}</span><p>{step.text}</p><EvidenceReferences ids={step.evidence_command_ids} context={context} /></li>)}</ol> : <p>尚未生成步骤清单。</p>}<p className="context-caveat">步骤状态由模型更新，不会代替命令审批，也不证明远端操作成功。</p></section>
    {context.truncated && <p className="notice warning">摘要或证据已达到保留上限；仅保留有界上下文，不是完整终端记录。</p>}
  </details>;
}
