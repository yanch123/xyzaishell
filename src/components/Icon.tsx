import type { CSSProperties } from 'react';
type IconName = 'terminal' | 'plus' | 'chevron' | 'message' | 'settings' | 'server' | 'link' | 'disconnect' | 'close' | 'arrow' | 'shield' | 'key' | 'history' | 'check' | 'info' | 'spark' | 'command' | 'lock';
const paths: Record<IconName, string> = {
  terminal: 'm5 7 5 5-5 5M12 17h7', plus: 'M12 5v14M5 12h14', chevron: 'm9 5 7 7-7 7',
  message: 'M20 11.5a8 8 0 0 1-8 8H4v-7a8 8 0 0 1 16-1ZM8 9h8M8 13h5',
  settings: 'M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8ZM12 3v3M12 18v3M3 12h3M18 12h3M5.6 5.6l2.1 2.1m8.6 8.6 2.1 2.1M5.6 18.4l2.1-2.1m8.6-8.6 2.1-2.1',
  server: 'M4 4h16v6H4ZM4 14h16v6H4ZM7 7h.01M7 17h.01M15 7h2M15 17h2',
  link: 'm10 13 4-4M8 16l-1 1a4 4 0 0 1-6-6l4-4a4 4 0 0 1 6 0M16 8l1-1a4 4 0 0 1 6 6l-4 4a4 4 0 0 1-6 0',
  disconnect: 'M9 3v7M15 3v7M6 10h12v3a6 6 0 0 1-12 0v-3ZM12 19v3',
  close: 'm6 6 12 12M6 18 18 6', arrow: 'M12 19V5m-6 6 6-6 6 6',
  shield: 'm12 3 8 3v6c0 5-8 9-8 9s-8-4-8-9V6l8-3Zm-4 9 3 3 5-6',
  key: 'M14 4a5 5 0 1 0 5 5 5 5 0 0 0-5-5ZM10 13l-7 7M3 16l4 4',
  history: 'M3 11a9 9 0 1 1 2 7M3 5v6h6M12 7v5l3 2',
  check: 'm5 12 4 4L19 6', info: 'M12 16v-4m0-4h.01M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z',
  spark: 'm12 3 2.6 6.4L21 12l-6.4 2.6L12 21l-2.6-6.4L3 12l6.4-2.6L12 3Z',
  command: 'M9 7V5a2 2 0 1 0-2 2h10a2 2 0 1 0-2-2v14a2 2 0 1 0 2-2H7a2 2 0 1 0 2 2V7Z',
  lock: 'M6 10h12v11H6ZM8 10V7a4 4 0 0 1 8 0v3M12 14v3',
};
export function Icon({ name, size = 18, style, className = '' }: { name: IconName; size?: number; style?: CSSProperties; className?: string }) {
  return <svg aria-hidden="true" width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" style={style} className={className}><path d={paths[name]} /></svg>;
}
