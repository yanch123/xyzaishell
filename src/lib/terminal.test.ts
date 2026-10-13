import { describe, expect, it, vi } from 'vitest';
import { MAX_INPUT_FRAME, MAX_INPUT_QUEUE, fromBase64, splitInput, terminalBytes, TerminalInputQueue, toBase64 } from './terminal';
const session = { session_id: 'session-1', generation: '1' };
const settle = () => new Promise<void>(resolve => setTimeout(resolve, 0));
describe('terminal bytes and bounded input', () => {
  it('round trips UTF-8, controls and non-ASCII binary without string coercion', () => {
    const bytes = terminalBytes('你好 👋\r\n\x1b[31m');
    expect([...fromBase64(toBase64(bytes))]).toEqual([...bytes]);
    expect(terminalBytes('\x80\xff', true)).toEqual(new Uint8Array([128,255]));
  });
  it('splits a paste into at most 16 KiB frames without losing bytes', () => {
    const bytes = terminalBytes('中文'.repeat(12000));
    const frames = splitInput(bytes);
    expect(frames.every(frame => frame.length <= MAX_INPUT_FRAME)).toBe(true);
    expect(frames.flatMap(frame => [...frame])).toEqual([...bytes]);
  });
  it('serializes sends in FIFO order, including simultaneous paste and typing', async () => {
    let unblock!: () => void;
    const send = vi.fn().mockImplementationOnce(() => new Promise<void>(r => { unblock = r; })).mockResolvedValue(undefined);
    const queue = new TerminalInputQueue(session, send, vi.fn());
    queue.enqueue(terminalBytes('one')); queue.enqueue(terminalBytes('two'));
    expect(send).toHaveBeenCalledTimes(1);
    unblock(); await settle();
    expect(send.mock.calls.map(call => new TextDecoder().decode(fromBase64(call[1])))).toEqual(['one','two']);
  });
  it('rejects an entire oversize paste before sending any bytes', () => {
    const send = vi.fn(); const failure = vi.fn();
    const queue = new TerminalInputQueue(session, send, failure);
    expect(queue.enqueue(new Uint8Array(MAX_INPUT_QUEUE + 1))).toBe(false);
    expect(send).not.toHaveBeenCalled(); expect(failure).toHaveBeenCalledOnce();
  });
  it('clears queued input on failure without retrying an uncertain frame', async () => {
    const send = vi.fn().mockRejectedValue({ code: 'DELIVERY_UNKNOWN' }); const failure = vi.fn();
    const queue = new TerminalInputQueue(session, send, failure);
    queue.enqueue(new Uint8Array(MAX_INPUT_FRAME + 100)); await settle();
    expect(send).toHaveBeenCalledOnce(); expect(failure).toHaveBeenCalledOnce();
  });
  it('discards queued frames on disconnect and never targets a new session', async () => {
    let unblock!: () => void;
    const send = vi.fn().mockImplementation(() => new Promise<void>(r => { unblock = r; }));
    const queue = new TerminalInputQueue(session, send, vi.fn());
    queue.enqueue(new Uint8Array(MAX_INPUT_FRAME + 100)); queue.dispose(); unblock(); await settle();
    expect(send).toHaveBeenCalledOnce(); expect(send.mock.calls[0][0]).toEqual(session);
    expect(queue.enqueue(terminalBytes('no'))).toBe(false);
  });
});
