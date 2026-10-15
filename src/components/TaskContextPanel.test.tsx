import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { task } from '../test/agentFixtures';
import type { TaskContext } from '../lib/agent';
import { TaskContextPanel } from './TaskContextPanel';
function context(overrides: Partial<TaskContext> = {}): TaskContext {
  const current = task();
  return { task_id: current.task_id, run_id: current.run_id, session: current.session, summary: '正在核查服务状态',
    confirmed_facts: [{ command_id: 'cmd-1', kind: 'foreground_returned', exit_code: 0 }],
    findings: [{ id: 'fact-1', text: '输出报告版本 1.2', evidence_command_ids: ['cmd-1'] }],
    hypotheses: [{ id: 'guess-1', text: '可能是端口冲突，尚未核实', evidence_command_ids: [] }],
    remaining_steps: [{ id: 'step-1', text: '检查监听端口', status: 'pending', evidence_command_ids: [] }],
    evidence: [{ command_id: 'cmd-1', command_text: 'service --version', delivery: 'write_accepted', command_completed: true, exit_code: 0, output_excerpt: '<script>unsafe()</script>', output_truncated: false, output_gap: false, untrusted: true }],
    truncated: false, ...overrides };
}
describe('task context display', () => {
  it('distinguishes evidence findings, hypotheses, host events and model step status without execution controls', () => {
    const view = render(<TaskContextPanel task={task({ task_context: context() })} />);
    fireEvent.click(screen.getByText(/已确认事实 \/ 剩余步骤/));
    expect(screen.getByText('输出报告版本 1.2')).toBeDefined();
    expect(screen.getByText('可能是端口冲突，尚未核实')).toBeDefined();
    expect(screen.getByText(/尚非独立核验/)).toBeDefined();
    expect(screen.getByText('检查监听端口')).toBeDefined();
    expect(screen.getByText(/不代表业务目标完成/)).toBeDefined();
    expect(view.container.querySelector('script')).toBeNull();
    expect(view.container.querySelector('button')).toBeNull();
    expect(view.container.textContent).toContain('<script>unsafe()</script>');
  });
  it('does not show mismatched run, task or session context', () => {
    const view = render(<TaskContextPanel task={task()} />);
    expect(view.container.textContent).toBe('');
    for (const override of [{ run_id: 'old' }, { task_id: 'other' }, { session: { session_id: 'new', generation: '2' } }]) {
      view.rerender(<TaskContextPanel task={task({ task_context: context(override) })} />);
      expect(view.container.textContent).toBe('');
    }
  });
  it('labels incomplete evidence, missing references, and model-completed steps', () => {
    const c = context({ truncated: true });
    c.evidence[0].output_gap = true;
    c.findings[0].evidence_command_ids.push('missing');
    c.remaining_steps[0].status = 'completed';
    render(<TaskContextPanel task={task({ task_context: c })} />);
    expect(screen.getByText(/输出不完整/)).toBeDefined();
    expect(screen.getByText(/来源片段已不在/)).toBeDefined();
    expect(screen.getByText('已完成（模型标记）')).toBeDefined();
    expect(screen.getByText(/不是完整终端记录/)).toBeDefined();
  });
});
