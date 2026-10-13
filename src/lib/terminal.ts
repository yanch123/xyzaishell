import type { SessionRef } from './types';
export const MAX_INPUT_FRAME = 16 * 1024;
export const MAX_INPUT_QUEUE = 256 * 1024;
export const MAX_OUTPUT_PENDING = 4 * 1024 * 1024;

export function toBase64(bytes: Uint8Array): string {
  let value = '';
  for (let start = 0; start < bytes.length; start += 8192) value += String.fromCharCode(...bytes.subarray(start, start + 8192));
  return btoa(value);
}
export function fromBase64(value: string): Uint8Array {
  return Uint8Array.from(atob(value), c => c.charCodeAt(0));
}
export function terminalBytes(value: string, binary = false): Uint8Array {
  return binary ? Uint8Array.from(value, c => c.charCodeAt(0) & 255) : new TextEncoder().encode(value);
}
export function splitInput(bytes: Uint8Array): Uint8Array[] {
  const frames: Uint8Array[] = [];
  for (let start = 0; start < bytes.length; start += MAX_INPUT_FRAME) frames.push(bytes.slice(start, start + MAX_INPUT_FRAME));
  return frames;
}
/** Bounded FIFO. A failed frame and its remaining paste are never replayed. */
export class TerminalInputQueue {
  private frames: Uint8Array[] = [];
  private queuedBytes = 0;
  private sending = false;
  private disposed = false;
  constructor(private session: SessionRef, private send: (session: SessionRef, data: string) => Promise<unknown>, private onFailure: (error: unknown) => void) {}
  enqueue(bytes: Uint8Array): boolean {
    if (this.disposed) return false;
    if (bytes.length > MAX_INPUT_QUEUE - this.queuedBytes) {
      this.onFailure({ code: 'BACKPRESSURE', safe_message: '输入队列已满，本次输入未发送。请等待后再输入；不要直接重放整段命令。' });
      return false;
    }
    this.frames.push(...splitInput(bytes));
    this.queuedBytes += bytes.length;
    void this.drain();
    return true;
  }
  dispose(): void { this.disposed = true; this.frames = []; this.queuedBytes = 0; }
  private async drain(): Promise<void> {
    if (this.sending || this.disposed) return;
    this.sending = true;
    while (this.frames.length && !this.disposed) {
      const frame = this.frames.shift()!;
      try {
        await this.send(this.session, toBase64(frame));
      } catch (error) {
        if (!this.disposed) {
          this.frames = [];
          this.queuedBytes = 0;
          this.onFailure(error);
        }
        this.sending = false;
        return;
      }
      if (!this.disposed) this.queuedBytes -= frame.length;
    }
    this.sending = false;
  }
}
