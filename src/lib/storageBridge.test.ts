import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { storageBridge } from './storageBridge';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue({}) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));
describe('profile IPC contracts', () => {
  beforeEach(() => vi.clearAllMocks());
  it('binds a one-attempt credential to exact ID and revision with explicit remembering', async () => {
    const size = { cols: 80, rows: 24 };
    await storageBridge.connectProfile('synthetic-id', '17', size, { temporaryCredential: 'synthetic-only-once' });
    expect(invoke).toHaveBeenCalledExactlyOnceWith('connect_saved_profile', { connectionId: 'synthetic-id', profileRevision: '17', size, temporaryCredential: 'synthetic-only-once', rememberCredential: false });
  });
  it('never requests a global policy downgrade or a process-wide secret cache', () => {
    expect(storageBridge).not.toHaveProperty('sessionMode'); expect(storageBridge).not.toHaveProperty('sessionCredential');
  });
  it('keeps deletion scoped and revision checked', async () => {
    await storageBridge.deleteProfile('synthetic-id', '18'); expect(invoke).toHaveBeenCalledExactlyOnceWith('delete_connection_profile', { connectionId: 'synthetic-id', expectedRevision: '18' });
  });
});
