import { useRef, useState } from 'react';
import type { TerminalTaskSnapshot, ActionOutputPage } from '../lib/agent';
import type { TerminalAgentController } from '../hooks/useTerminalAgent';
import { safeError } from '../lib/types';

/** Explicit local reads only. This panel never runs a command or sends to a model. */
export function ActionOutput({ task, actionId, agent, connected }: { task: TerminalTaskSnapshot; actionId: string; agent: TerminalAgentController; connected: boolean }) {
  const [page, setPage] = useState<ActionOutputPage | null>(null);
  const [offsets, setOffsets] = useState<number[]>([]);
  const [offset, setOffset] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const lock = useRef(false);
  const identity = useRef(''); identity.current = `${task.run_id}:${actionId}:${connected}`;
  const read = async (next: number, previous: number[]) => {
    if (!connected || lock.current) return;
    const expected = identity.current;
    lock.current = true; setBusy(true); setError(null);
    try {
      const result = await agent.readOutput(task.task_id, task.run_id, actionId, next);
      if (identity.current !== expected) return;
      setPage(result); setOffset(next); setOffsets(previous);
    } catch (failure) { if (identity.current === expected) setError(safeError(failure)); }
    finally { lock.current = false; setBusy(false); }
  };
  return <details className="action-output"><summary>查看输出</summary>
    <p>读取已保留的输出，不会重新运行命令。</p>
    {!page && <button className="button secondary" disabled={!connected || busy} onClick={() => void read(0, [])}>{busy ? '读取中…' : '读取输出'}</button>}
    {page && <>
      {(page.gap || page.truncated) && <p className="notice warning">仅保留部分输出，较早内容可能已丢失。</p>}
      <pre>{page.text || '此页没有保留内容。'}</pre>
      <div className="action-buttons">
        <button className="button secondary" disabled={!connected || busy || !offsets.length} onClick={() => void read(offsets.at(-1)!, offsets.slice(0, -1))}>上一页</button>
        <button className="button secondary" disabled={!connected || busy || !page.has_more || page.next_offset <= offset} onClick={() => void read(page.next_offset, [...offsets, offset].slice(-64))}>下一页</button>
        {!page.complete && !page.has_more && <button className="button secondary" disabled={!connected || busy} onClick={() => void read(offset, offsets)}>刷新输出</button>}
        {page.gap && page.earliest_offset > offset && <button className="button secondary" disabled={!connected || busy} onClick={() => void read(page.earliest_offset, [])}>从可用内容开始</button>}
      </div>
    </>}
    {error && <p className="inline-error" role="alert">{error}</p>}
  </details>;
}
