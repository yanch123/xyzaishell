import { describe, expect, it } from 'vitest';
import { isActiveSession, safeError, sameSession } from './types';
describe('safe session boundaries', () => {
  it('does not render raw exception text or credentials', () => {
    expect(safeError('password=TEST_SECRET')).not.toContain('TEST_SECRET');
    expect(safeError(new Error('TEST_SECRET'))).not.toContain('TEST_SECRET');
    expect(safeError({ code: 'SSH_AUTH_FAILED', safe_message: 'SSH 认证失败' })).toBe('SSH 认证失败');
  });
  it('rejects stale connection generations even with matching ID', () => {
    expect(sameSession({session_id:'a',generation:'1'},{session_id:'a',generation:'2'})).toBe(false);
    expect(sameSession({session_id:'a',generation:'1'},{session_id:'a',generation:'1'})).toBe(true);
    expect(sameSession(null,null)).toBe(false);
  });
  it('keeps connecting, trust and closing states busy', () => {
    expect(isActiveSession('connecting')).toBe(true); expect(isActiveSession('awaiting_host_key')).toBe(true);
    expect(isActiveSession('closing')).toBe(true); expect(isActiveSession('disconnected')).toBe(false);
  });
});
