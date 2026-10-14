import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { IntegrationPanel } from './IntegrationPanel';
import { useShellIntegration } from '../hooks/useShellIntegration';
import { integrationBridge } from '../lib/integrationBridge';
import type { IntegrationStatus } from '../lib/integrationBridge';
import type { SessionSnapshot } from '../lib/types';
vi.mock('../lib/integrationBridge',()=>({integrationBridge:{status:vi.fn(),install:vi.fn()}}));
const snapshot:SessionSnapshot={state:'connected',session:{session_id:'s',generation:'1'},host:'server',username:'alice',port:22};
const status:IntegrationStatus={phase:'unintegrated',session:snapshot.session,revision:0,sequence:0,shadowed_commands:[]};
function Harness(){const integration=useShellIntegration(true,snapshot);return <IntegrationPanel integration={integration} snapshot={snapshot} onClose={vi.fn()}/>;}
describe('current-shell installation',()=>{
  beforeEach(()=>{vi.resetAllMocks();vi.mocked(integrationBridge.status).mockResolvedValue(status);});
  it('requires idle-shell consent, never installs on connect, and locks repeated clicks',async()=>{
    let resolve!:(s:IntegrationStatus)=>void;vi.mocked(integrationBridge.install).mockReturnValue(new Promise(r=>resolve=r));
    render(<Harness/>);await screen.findByText('尚未启用');
    expect(integrationBridge.install).not.toHaveBeenCalled();
    expect((screen.getByRole('button',{name:'启用命令识别'}) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole('checkbox'));const install=screen.getByRole('button',{name:'启用命令识别'});
    fireEvent.click(install);fireEvent.click(install);expect(integrationBridge.install).toHaveBeenCalledExactlyOnceWith(snapshot.session);
    await act(async()=>resolve({...status,phase:'ready',revision:1}));await screen.findByText('命令识别已就绪');
    expect(screen.queryByRole('button',{name:'启用命令识别'})).toBeNull();
  });
  it('does not replay an ambiguous install after failure or panel reopening',async()=>{
    vi.mocked(integrationBridge.install).mockRejectedValue(new Error('sensitive payload'));
    render(<Harness/>);await screen.findByText('尚未启用');fireEvent.click(screen.getByRole('checkbox'));fireEvent.click(screen.getByRole('button',{name:'启用命令识别'}));
    await waitFor(()=>expect(screen.getByText(/启用结果需要核对/)).toBeDefined());
    expect(screen.queryByText('sensitive payload')).toBeNull();expect(integrationBridge.install).toHaveBeenCalledOnce();
    expect((screen.getByRole('checkbox') as HTMLInputElement).disabled).toBe(true);
  });
});
