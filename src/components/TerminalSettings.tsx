import { useEffect, useState } from 'react';
import { useTerminalSettings } from '../hooks/useTerminalSettings';
import { DEFAULT_TERMINAL_SETTINGS } from '../lib/terminalSettings';

export function TerminalSettings() {
  const { settings, update, saveFailed } = useTerminalSettings();
  const [fontDraft, setFontDraft] = useState(String(settings.fontSize));
  useEffect(() => setFontDraft(String(settings.fontSize)), [settings.fontSize]);
  return <section className="terminal-settings" aria-label="终端显示设置">
    <h3>终端显示</h3>
    <label className="field">字体大小（10–28 px）<input aria-label="终端字体大小" type="number" min={10} max={28} step={1} value={fontDraft} onChange={event => { const value = event.target.value; setFontDraft(value); const size = Number(value); if (value && Number.isInteger(size) && size >= 10 && size <= 28) update({ ...settings, fontSize: size }); }} onBlur={() => { const value = fontDraft ? Math.max(10, Math.min(28, Math.round(Number(fontDraft)))) : settings.fontSize; const size = Number.isFinite(value) ? value : settings.fontSize; setFontDraft(String(size)); update({ ...settings, fontSize: size }); }} /></label>
    <label className="field">回滚行数（500–20000）<select aria-label="终端回滚行数" value={settings.scrollback} onChange={event => update({ ...settings, scrollback: Number(event.target.value) })}>{[...new Set([500, 1000, 5000, 10000, 20000, settings.scrollback])].sort((a, b) => a - b).map(value => <option value={value} key={value}>{value}</option>)}</select></label>
    <p>仅保存显示偏好。缩小回滚行数会丢弃更早的屏幕内容；不会清除远端输出或操作日志。</p>
    <button className="button secondary" onClick={() => update(DEFAULT_TERMINAL_SETTINGS)}>恢复终端默认设置</button>
    {saveFailed && <p role="alert" className="inline-error">偏好无法持久保存，本次窗口仍已应用。关闭后可能恢复默认值。</p>}
  </section>;
}
