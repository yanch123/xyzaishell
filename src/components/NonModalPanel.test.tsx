import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { NonModalPanel } from './NonModalPanel';
describe('nonmodal workspace inspectors',()=>{
  it('has accessible identity without focus capture or a backdrop',()=>{
    const outside=document.createElement('button');document.body.append(outside);outside.focus();
    const close=vi.fn();const view=render(<NonModalPanel title="Settings" onClose={close}><input aria-label="Draft" /></NonModalPanel>);
    expect(screen.getByRole('dialog',{name:'Settings'}).getAttribute('aria-modal')).toBe('false');
    expect(document.activeElement).toBe(outside);expect(document.querySelector('.modal-backdrop')).toBeNull();
    fireEvent.keyDown(screen.getByRole('textbox'),{key:'Escape'});expect(close).toHaveBeenCalledOnce();
    view.unmount();outside.remove();
  });
});
