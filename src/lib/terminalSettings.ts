export type TerminalSettings = { fontSize: number; scrollback: number };
export const DEFAULT_TERMINAL_SETTINGS: TerminalSettings = { fontSize: 14, scrollback: 5000 };
export const TERMINAL_SETTINGS_KEY = 'xyzaishell.terminal.v1';
export const TERMINAL_SETTINGS_EVENT = 'xyzaishell:terminal-settings';
const integer = (value: unknown, fallback: number, min: number, max: number) => typeof value === 'number' && Number.isFinite(value) ? Math.max(min, Math.min(max, Math.round(value))) : fallback;
export function normalizeTerminalSettings(value: unknown): TerminalSettings {
  const candidate = value && typeof value === 'object' ? value as Partial<TerminalSettings> : {};
  return { fontSize: integer(candidate.fontSize, 14, 10, 28), scrollback: integer(candidate.scrollback, 5000, 500, 20000) };
}
export function readTerminalSettings(): TerminalSettings {
  try {
    const stored = localStorage.getItem(TERMINAL_SETTINGS_KEY);
    if (!stored || stored.length > 256) return { ...DEFAULT_TERMINAL_SETTINGS };
    const parsed = JSON.parse(stored);
    return parsed.version === 1 ? normalizeTerminalSettings(parsed) : { ...DEFAULT_TERMINAL_SETTINGS };
  } catch { return { ...DEFAULT_TERMINAL_SETTINGS }; }
}
export function saveTerminalSettings(settings: TerminalSettings): boolean {
  try {
    localStorage.setItem(TERMINAL_SETTINGS_KEY, JSON.stringify({ version: 1, ...normalizeTerminalSettings(settings) }));
    return true;
  } catch { return false; }
}
