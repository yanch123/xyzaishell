import { beforeEach, describe, expect, it, vi } from 'vitest';
import { DEFAULT_TERMINAL_SETTINGS, normalizeTerminalSettings, readTerminalSettings, saveTerminalSettings, TERMINAL_SETTINGS_KEY } from './terminalSettings';
beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });
describe('non-sensitive terminal preference persistence', () => {
  it('clamps numbers and ignores invalid types/non-finite values', () => {
    expect(normalizeTerminalSettings({ fontSize: 999, scrollback: -2 })).toEqual({ fontSize: 28, scrollback: 500 });
    expect(normalizeTerminalSettings({ fontSize: '22', scrollback: Infinity })).toEqual(DEFAULT_TERMINAL_SETTINGS);
    expect(normalizeTerminalSettings({ fontSize: 15.6, scrollback: 3000.5 })).toEqual({ fontSize: 16, scrollback: 3001 });
  });
  it('recovers corrupt, unsupported and oversized settings without failing terminal initialization', () => {
    for (const value of ['{invalid', 'null', '{"version":2,"fontSize":28}', ' '.repeat(257)]) {
      localStorage.setItem(TERMINAL_SETTINGS_KEY, value); expect(readTerminalSettings()).toEqual(DEFAULT_TERMINAL_SETTINGS);
    }
  });
  it('stores only bounded display preferences and reports unavailable storage', () => {
    expect(saveTerminalSettings({ fontSize: 18, scrollback: 1000 })).toBe(true);
    expect(JSON.parse(localStorage.getItem(TERMINAL_SETTINGS_KEY)!)).toEqual({ version: 1, fontSize: 18, scrollback: 1000 });
    expect(readTerminalSettings()).toEqual({ fontSize: 18, scrollback: 1000 });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('quota'); });
    expect(saveTerminalSettings(DEFAULT_TERMINAL_SETTINGS)).toBe(false);
  });
});
