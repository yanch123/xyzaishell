import { Modal } from './Modal';
import { Icon } from './Icon';
export function SettingsDialog({ onClose }: { onClose: () => void }) {
  return <Modal title="工作空间设置" eyebrow="XYZAI SHELL · 0.1.0" onClose={onClose} className="settings-modal">
    <div className="settings-section"><div className="settings-icon"><Icon name="terminal" /></div><div><h3>SSH 终端 <span className="feature-badge">当前阶段</span></h3><p>密码认证、Linux 私钥认证、主机指纹核验、交互终端和窗口尺寸同步。每个进程只连接一台服务器。</p></div></div>
    <div className="settings-section"><div className="settings-icon"><Icon name="spark" /></div><div><h3>模型配置 <span className="subtle-badge">M2 待接入</span></h3><p>后续使用独立的模型配置文件，设置 Base URL、API Key 和模型名称。当前不会读取模型 Key，也不会发送聊天或终端内容。</p><div className="future-fields"><span>Base URL <b>尚未加载</b></span><span>模型 <b>尚未配置</b></span></div></div></div>
    <div className="settings-section"><div className="settings-icon"><Icon name="shield" /></div><div><h3>动作与授权 <span className="subtle-badge">M3 待接入</span></h3><p>未来的 AI 命令会进入同一个可见终端。高风险动作先审核，未知状态暂停当前步骤。当前没有 AI 终端写入能力。</p></div></div>
    <div className="notice"><Icon name="info" /><span>未来 AI 活动期间，手工输入仍然可用。请自行安排输入时机；应用不会自动抢占、暂停或恢复 AI。</span></div>
    <div className="modal-footer"><button className="button primary" onClick={onClose}>知道了</button></div>
  </Modal>;
}
