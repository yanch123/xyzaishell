import { useId } from 'react';
import type { ReactNode } from 'react';
import { Icon } from './Icon';

/** A workspace inspector: no backdrop, focus trap, autofocus, or global inert state. */
export function NonModalPanel({ title, eyebrow, children, onClose, className = '' }: { title: string; eyebrow?: string; children: ReactNode; onClose: () => void; className?: string }) {
  const titleId = useId();
  return <div className="nonmodal-host">
    <section className={`modal nonmodal-panel ${className}`} role="dialog" aria-modal="false" aria-labelledby={titleId} onKeyDown={event => {
      if (event.key === 'Escape' && !event.defaultPrevented) { event.preventDefault(); event.stopPropagation(); onClose(); }
    }}>
      <div className="modal-header"><div>{eyebrow && <div className="eyebrow">{eyebrow}</div>}<h2 id={titleId}>{title}</h2></div><button className="icon-button" type="button" aria-label="关闭对话框" onClick={onClose}><Icon name="close" /></button></div>
      {children}
    </section>
  </div>;
}
