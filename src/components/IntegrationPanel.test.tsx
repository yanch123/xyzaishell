import { render, screen } from '@testing-library/react';
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
describe('automatic current-shell preparation',()=>{
  beforeEach(()=>{vi.resetAllMocks();vi.mocked(integrationBridge.status).mockResolvedValue(status);});
  it('shows actual state without a manual readiness checkbox or startup injection',async()=>{
    render(<Harness/>);await screen.findByText('仅手动 · 尚无 Shell 就绪证据');
    expect(integrationBridge.install).not.toHaveBeenCalled();
    expect(screen.queryByRole('checkbox')).toBeNull();
    expect(screen.queryByRole('button',{name:'启用命令识别'})).toBeNull();
    expect(screen.getByText(/使用非登录 Shell/)).toBeDefined();
  });
  it('distinguishes unknown from busy and shows precise backend errors',async()=>{
    vi.mocked(integrationBridge.status).mockResolvedValue({...status,phase:'unknown',error:{code:'SHELL_STARTUP_TIMEOUT',safe_message:'Startup needs user interaction.',retry:'after_user_action'}});
    render(<Harness/>);await screen.findByText('仅手动 · 状态未知，需核对');
    expect(screen.getByText('Startup needs user interaction.')).toBeDefined();
    expect(integrationBridge.install).not.toHaveBeenCalled();
  });
});
