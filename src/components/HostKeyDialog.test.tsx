import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { HostKeyDialog } from './HostKeyDialog';
const challenge = {challenge_id:'c1',session:{session_id:'s1',generation:'1'},host:'test.example',port:22,algorithm:'ssh-ed25519',fingerprint:'SHA256:TEST_FINGERPRINT',changed:false};
describe('host identity confirmation', () => {
  it('requires explicit verification and binds approval to the exact challenge', async () => {
    const decide = vi.fn().mockResolvedValue(undefined); const user = userEvent.setup();
    render(<HostKeyDialog challenge={challenge} onDecide={decide} />);
    expect((screen.getByRole('button',{name:'信任并继续连接'}) as HTMLButtonElement).disabled).toBe(true);
    await user.click(screen.getByRole('checkbox')); await user.click(screen.getByRole('button',{name:'信任并继续连接'}));
    expect(decide).toHaveBeenCalledExactlyOnceWith(challenge,true);
  });
  it('warns about changed keys and rejects rather than trusts on Escape', async () => {
    const decide = vi.fn().mockResolvedValue(undefined); const user = userEvent.setup();
    const changed = {...challenge,changed:true,previous_fingerprint:'SHA256:OLD'};
    render(<HostKeyDialog challenge={changed} onDecide={decide} />);
    expect(screen.getByText('警告：服务器身份已变化')).toBeDefined(); expect(screen.getByText('SHA256:OLD')).toBeDefined();
    await user.keyboard('{Escape}'); expect(decide).toHaveBeenCalledExactlyOnceWith(changed,false);
  });
});
