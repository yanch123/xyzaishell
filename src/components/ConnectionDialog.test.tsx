import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ConnectionDialog } from './ConnectionDialog';
const size = { cols: 80, rows: 24 };
describe('dedicated SSH authentication form', () => {
  it('requires explicit session-only consent and clears credentials after submit', async () => {
    const connect = vi.fn().mockResolvedValue(false); const user = userEvent.setup();
    render(<ConnectionDialog available size={size} onConnect={connect} onClose={vi.fn()} />);
    await user.type(screen.getByLabelText('主机地址'),'test.example');
    await user.type(screen.getByLabelText('用户名'),'tester');
    await user.type(screen.getByLabelText('登录密码'),'TEST_SECRET');
    expect((screen.getByRole('button',{name:'连接服务器'}) as HTMLButtonElement).disabled).toBe(true);
    await user.click(screen.getByRole('checkbox'));
    await user.click(screen.getByRole('button',{name:'连接服务器'}));
    await waitFor(() => expect(connect).toHaveBeenCalledOnce());
    expect(connect.mock.calls[0][0]).toEqual({host:'test.example',port:22,username:'tester',session_only:true,size,auth:{kind:'password',password:'TEST_SECRET'}});
    expect((screen.getByLabelText('登录密码') as HTMLInputElement).value).toBe('');
    expect(localStorage.length).toBe(0);
  });
  it('accepts an absolute key path and optional passphrase with no password retention', async () => {
    const connect = vi.fn().mockResolvedValue(true); const close = vi.fn(); const user = userEvent.setup();
    render(<ConnectionDialog available size={size} onConnect={connect} onClose={close} />);
    await user.type(screen.getByLabelText('主机地址'),'127.0.0.1'); await user.type(screen.getByLabelText('用户名'),'test');
    await user.click(screen.getByRole('button',{name:'私钥文件'}));
    await user.type(screen.getByLabelText('本机私钥绝对路径'),'/tmp/test_id');
    await user.click(screen.getByRole('checkbox')); await user.click(screen.getByRole('button',{name:'连接服务器'}));
    await waitFor(() => expect(close).toHaveBeenCalledOnce());
    expect(connect.mock.calls[0][0].auth).toEqual({kind:'private_key',private_key_path:'/tmp/test_id'});
  });
  it('rejects invalid ports and relative private-key paths without invoking native code', async () => {
    const connect = vi.fn(); const user = userEvent.setup();
    render(<ConnectionDialog available size={size} onConnect={connect} onClose={vi.fn()} />);
    await user.type(screen.getByLabelText('主机地址'),'host'); await user.type(screen.getByLabelText('用户名'),'tester');
    await user.click(screen.getByRole('button',{name:'私钥文件'})); await user.type(screen.getByLabelText('本机私钥绝对路径'),'relative_key');
    await user.click(screen.getByRole('checkbox')); await user.click(screen.getByRole('button',{name:'连接服务器'}));
    expect(connect).not.toHaveBeenCalled(); expect(screen.getByRole('alert').textContent).toContain('绝对路径');
    fireEvent.change(screen.getByLabelText('端口'),{target:{value:'99999'}}); await user.click(screen.getByRole('button',{name:'连接服务器'}));
    expect(connect).not.toHaveBeenCalled(); expect(screen.getByRole('alert').textContent).toContain('65535');
  });
  it('never connects in a plain browser preview', () => {
    render(<ConnectionDialog available={false} size={size} onConnect={vi.fn()} onClose={vi.fn()} />);
    expect(screen.getByText(/网页预览无法连接/)).toBeDefined();
    expect((screen.getByRole('button',{name:'连接服务器'}) as HTMLButtonElement).disabled).toBe(true);
  });
});
