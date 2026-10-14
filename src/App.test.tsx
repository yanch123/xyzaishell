import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SessionSnapshot } from './lib/types';
import { modelConfig } from './test/modelFixtures';
const mocks=vi.hoisted(()=>({ssh:vi.fn(),model:vi.fn(),agent:vi.fn(),launch:vi.fn(),target:vi.fn(),snapshot:vi.fn(),historyStatus:vi.fn(),historyListen:vi.fn(),logStatus:vi.fn(),logListen:vi.fn(),logQuery:vi.fn(),profiles:vi.fn()}));
vi.mock('./hooks/useSshSession',()=>({useSshSession:mocks.ssh}));
vi.mock('./hooks/useModelChat',()=>({useModelChat:mocks.model}));
vi.mock('./hooks/useTerminalAgent',()=>({useTerminalAgent:mocks.agent}));
vi.mock('./hooks/useShellIntegration',()=>({useShellIntegration:()=>({status:null,busy:false,error:null,uncertain:false,install:vi.fn()})}));
vi.mock('./lib/windowBridge',()=>({windowBridge:{launch:mocks.launch,target:mocks.target}}));
vi.mock('./lib/bridge',()=>({sshBridge:{snapshot:mocks.snapshot}}));
vi.mock('./lib/storageBridge',()=>({historyBridge:{status:mocks.historyStatus,listen:mocks.historyListen},storageBridge:{profiles:mocks.profiles}}));
vi.mock('./lib/shellLogBridge',()=>({shellLogBridge:{status:mocks.logStatus,listen:mocks.logListen,query:mocks.logQuery}}));
vi.mock('./components/TerminalPane',()=>({TerminalPane:()=> <textarea aria-label="Manual terminal"/>}));
vi.mock('./components/ChatPane',()=>({ChatPane:()=> <section aria-label="Chat area"/>}));
vi.mock('./components/ProfileDialog',()=>({ProfileDialog:({onSwitchCurrent,onConnectInNewWindow}:{onSwitchCurrent:(id:string,revision:string)=>Promise<boolean>;onConnectInNewWindow:(id:string)=>Promise<boolean>})=><div role="dialog" aria-label="Profiles" aria-modal="false"><button onClick={()=>void onSwitchCurrent('p','1')}>Explicit switch</button><button onClick={()=>void onConnectInNewWindow('p')}>Other server</button></div>}));
import App from './App';
const session={session_id:'a',generation:'1'};
let ssh: {native:boolean;ready:boolean;snapshot:SessionSnapshot;historyUnavailable:boolean;error:null;challenge:null;setError:ReturnType<typeof vi.fn>;connect:ReturnType<typeof vi.fn>;connectSaved:ReturnType<typeof vi.fn>;disconnect:ReturnType<typeof vi.fn>;confirmHostKey:ReturnType<typeof vi.fn>;subscribeOutput:ReturnType<typeof vi.fn>;reportError:ReturnType<typeof vi.fn>;size:{current:{cols:number;rows:number}}};
beforeEach(()=>{
 vi.resetAllMocks();
 ssh={native:true,ready:true,snapshot:{state:'connected',session,host:'first',port:22,username:'alice'},historyUnavailable:false,error:null,challenge:null,setError:vi.fn(),connect:vi.fn(),connectSaved:vi.fn().mockResolvedValue(true),disconnect:vi.fn().mockResolvedValue(undefined),confirmHostKey:vi.fn(),subscribeOutput:vi.fn(),reportError:vi.fn(),size:{current:{cols:100,rows:30}}};
 mocks.ssh.mockImplementation(()=>ssh);
 mocks.model.mockReturnValue({native:true,ready:true,config:modelConfig,chats:[],pending:{},runningCount:0,configBusy:false,configTestBusy:false,configTest:null,creating:false,error:null,errors:{},confirmDestination:vi.fn(),saveConfig:vi.fn(),probe:vi.fn(),cancel:vi.fn()});
 mocks.agent.mockReturnValue({owner:null,pending:{},errors:{},cancel:vi.fn(),start:vi.fn()});
 mocks.profiles.mockResolvedValue({listing:{revision:'1',connections:[{id:'p',host:'next',port:22,username:'alice',label:'Next'}]}});
 mocks.launch.mockResolvedValue(undefined);mocks.target.mockResolvedValue({profile_id:null,new_connection:false});
 mocks.historyStatus.mockResolvedValue({enabled:false});mocks.historyListen.mockResolvedValue(vi.fn());
 mocks.logStatus.mockResolvedValue({state:'disabled',revision:'1'});mocks.logListen.mockResolvedValue(vi.fn());mocks.logQuery.mockResolvedValue({entries:[],next_before_seq:null,snapshot_max_seq:'0'});
});
describe('desktop workspace coordination',()=>{
 it('keeps the terminal interactive and retains a settings draft across inspector navigation',async()=>{
  render(<App/>);const terminal=screen.getByRole('textbox',{name:'Manual terminal'});terminal.focus();
  fireEvent.click(screen.getByRole('button',{name:'打开设置'}));
  const settings=screen.getByRole('dialog',{name:'工作空间设置'});expect(settings.getAttribute('aria-modal')).toBe('false');expect(terminal.closest('[inert]')).toBeNull();expect(document.activeElement).toBe(terminal);
  fireEvent.change(screen.getByRole('textbox',{name:/Base URL/}),{target:{value:'https://draft.example.test/v1'}});
  fireEvent.click(screen.getByRole('button',{name:'当前终端日志'}));await screen.findByRole('dialog',{name:'终端日志'});
  expect(screen.queryByRole('dialog',{name:'工作空间设置'})).toBeNull();expect(terminal.closest('[inert]')).toBeNull();
  fireEvent.click(screen.getByRole('button',{name:'打开设置'}));fireEvent.click(screen.getByRole('button',{name:'打开设置'}));
  expect(screen.getAllByRole('dialog',{name:'工作空间设置'})).toHaveLength(1);expect((screen.getByRole('textbox',{name:/Base URL/}) as HTMLInputElement).value).toBe('https://draft.example.test/v1');
  fireEvent.click(screen.getByRole('button',{name:'关闭设置'}));expect(screen.getByRole('group',{name:'处理未保存修改'})).toBeDefined();
 });
 it('starts another process once for repeated new-target clicks without disconnecting',async()=>{
  let finish!:()=>void;mocks.launch.mockReturnValue(new Promise<void>(r=>finish=r));render(<App/>);
  const create=screen.getAllByRole('button',{name:'新建连接'}).at(-1)!;fireEvent.click(create);fireEvent.click(create);
  expect(mocks.launch).toHaveBeenCalledExactlyOnceWith(null);expect(ssh.disconnect).not.toHaveBeenCalled();expect(ssh.connect).not.toHaveBeenCalled();
  await act(async()=>finish());
 });
 it('new-process failure leaves the existing session and surfaces a safe result',async()=>{
  mocks.launch.mockRejectedValue({code:'WINDOW_LAUNCH_FAILED',safe_message:'A new process could not start.'});render(<App/>);fireEvent.click(screen.getAllByRole('button',{name:'新建连接'}).at(-1)!);
  await screen.findByText(/A new process could not start/);expect(ssh.disconnect).not.toHaveBeenCalled();expect(ssh.connectSaved).not.toHaveBeenCalled();
 });
 it('explicit current-window switching requires confirmation and waits for disconnect evidence',async()=>{
  mocks.snapshot.mockResolvedValueOnce(ssh.snapshot).mockResolvedValueOnce({state:'disconnected',session});render(<App/>);fireEvent.click(screen.getByRole('button',{name:'连接管理'}));fireEvent.click(screen.getByRole('button',{name:'Explicit switch'}));
  expect((await screen.findByRole('dialog',{name:'在当前窗口切换连接'})).getAttribute('aria-modal')).toBe('true');expect(ssh.disconnect).not.toHaveBeenCalled();expect(ssh.connectSaved).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button',{name:'停止 AI 并继续'}));
  await waitFor(()=>expect(ssh.connectSaved).toHaveBeenCalledExactlyOnceWith('p','1',undefined));expect(ssh.disconnect).toHaveBeenCalledOnce();expect(mocks.snapshot).toHaveBeenCalledTimes(2);
 });
 it('cancelling a switch never changes session or starts a pending request',async()=>{
  render(<App/>);fireEvent.click(screen.getByRole('button',{name:'连接管理'}));fireEvent.click(screen.getByRole('button',{name:'Explicit switch'}));await screen.findByRole('dialog',{name:'在当前窗口切换连接'});fireEvent.click(screen.getByRole('button',{name:'取消'}));
  await waitFor(()=>expect(screen.queryByRole('dialog',{name:'在当前窗口切换连接'})).toBeNull());expect(ssh.disconnect).not.toHaveBeenCalled();expect(ssh.connectSaved).not.toHaveBeenCalled();
 });
});
