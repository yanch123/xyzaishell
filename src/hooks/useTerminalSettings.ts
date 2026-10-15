import { useEffect, useState } from 'react';
import { normalizeTerminalSettings, readTerminalSettings, saveTerminalSettings, TERMINAL_SETTINGS_EVENT, TERMINAL_SETTINGS_KEY } from '../lib/terminalSettings';
import type { TerminalSettings } from '../lib/terminalSettings';

export function useTerminalSettings() {
  const [settings, setSettings] = useState(readTerminalSettings);
  const [saveFailed, setSaveFailed] = useState(false);
  useEffect(() => {
    const local = (event: Event) => setSettings(normalizeTerminalSettings((event as CustomEvent).detail));
    const external = (event: StorageEvent) => { if (event.key === TERMINAL_SETTINGS_KEY || event.key === null) setSettings(readTerminalSettings()); };
    window.addEventListener(TERMINAL_SETTINGS_EVENT, local); window.addEventListener('storage', external);
    return () => { window.removeEventListener(TERMINAL_SETTINGS_EVENT, local); window.removeEventListener('storage', external); };
  }, []);
  const update = (next: TerminalSettings) => {
    const bounded = normalizeTerminalSettings(next);
    setSettings(bounded); setSaveFailed(!saveTerminalSettings(bounded));
    window.dispatchEvent(new CustomEvent(TERMINAL_SETTINGS_EVENT, { detail: bounded }));
  };
  return { settings, update, saveFailed };
}
