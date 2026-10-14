import { describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { shellLogBridge } from './shellLogBridge';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));
describe('shell log IPC contract', () => {
  it('uses dedicated native commands and explicit consent', async () => {
    const session = { session_id: 's', generation: '2' };
    await shellLogBridge.status(); expect(invoke).toHaveBeenLastCalledWith('get_shell_log_status');
    await shellLogBridge.start(session); expect(invoke).toHaveBeenLastCalledWith('start_shell_log', { session, consent: true, readinessConfirmed: true });
    await shellLogBridge.pause(true); expect(invoke).toHaveBeenLastCalledWith('set_shell_log_paused', { paused: true });
    const query = { contains: 'x', snapshot_max_seq: '999999999999999999' };
    await shellLogBridge.query(query); expect(invoke).toHaveBeenLastCalledWith('query_shell_log', { query });
    await shellLogBridge.export(query, 'json'); expect(invoke).toHaveBeenLastCalledWith('export_shell_log', { query, format: 'json' });
    await shellLogBridge.listen(vi.fn()); expect(listen).toHaveBeenCalledWith('shell-log:status', expect.any(Function));
  });
});
