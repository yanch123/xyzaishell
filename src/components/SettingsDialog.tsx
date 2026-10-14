import type { HistoryStatus } from '../lib/storageBridge';
import { HistorySettings } from './HistorySettings';
import { Modal } from './Modal';
import { Icon } from './Icon';
import type { ModelChatController } from '../hooks/useModelChat';

export function SettingsDialog({ onClose, model, onConsent, onTest, onHistoryStatus }: { onClose: () => void; model: ModelChatController; onConsent: () => void; onTest: () => Promise<void>; onHistoryStatus?: (status: HistoryStatus) => void }) {
  const { config } = model;
  return <Modal title="工作空间设置" eyebrow="XYZAI SHELL · v0.1" onClose={onClose} className="settings-modal">
    <div className="settings-section"><div className="settings-icon"><Icon name="spark" /></div><div className="settings-content"><h3>模型连接 <span className={config.ready ? 'feature-badge' : 'subtle-badge'}>{!model.native ? '网页预览' : config.ready ? '配置已加载' : '需要配置'}</span></h3>
      <p>OpenAI-compatible Chat Completions 流式聊天。默认聊天仅在当前应用进程内存保留；可在下方明确启用本机明文历史。服务提供方可能保留请求。</p>
      <dl className="model-details"><div><dt>配置路径</dt><dd className="selectable-path">{config.path ?? (model.native ? '正在读取本机配置路径…' : '桌面应用中显示实际路径')}</dd></div><div><dt>请求地址</dt><dd>{config.destination ?? '尚未加载'}</dd></div><div><dt>模型</dt><dd>{config.model ?? '尚未配置'}</dd></div><div><dt>聊天接收方确认</dt><dd>{config.consented ? '当前配置已确认' : '尚未确认'}</dd></div></dl>
      {config.warning && <div className="notice warning" role="status"><Icon name="info" /><span>{config.warning}</span></div>}
      {config.error && <div className="inline-error" role="alert">{config.error.safe_message}</div>}
      {model.error && <div className="inline-error" role="alert">{model.error}</div>}
      <div className="settings-actions"><button className="button secondary" disabled={!model.native || model.configBusy} onClick={() => void model.reloadConfig()}>{model.configBusy ? '正在读取…' : '重新加载模型配置'}</button>{config.ready && !config.consented && <button className="button secondary" onClick={onConsent} disabled={model.configBusy}>核对接收方</button>}{config.ready && config.consented && <button className="button secondary" onClick={() => void onTest()} disabled={model.creating || model.runningCount >= 4}>测试连接</button>}</div>
      {config.ready && <p className="field-hint">测试连接会向此接收方新发一个仅含“Reply with OK.”的测试聊天。仅检查聊天流式响应，不验证工具调用。</p>}
      <details className="config-guide" open={!config.ready}><summary>如何在聊天之外配置模型</summary><ol><li>在本机编辑器中打开上方路径的 model.toml，或把仓库的 config/examples/model.example.toml 复制到此位置。</li><li>在文件中设置 model.base_url、model.name 和 model.api_key。Base URL 为 API 根地址，例如 https://api.example.invalid/v1，不包含 /chat/completions。示例地址与占位值不能用于连接。</li><li>保存到私有本机目录；不要放入 Git、同步盘或共享目录。TOML 中的 Key 是明文，Linux 文件权限应为 0600，私有目录为 0700。</li><li>点击“重新加载模型配置”，核对地址和模型后再发送。更换 XYZAI_MODEL_CONFIG 指向的绝对路径需要重启应用。</li></ol><p>请勿把 API Key、SSH 密码、私钥或整份配置粘贴到聊天中。当前设置页不会读取或展示 Key。</p></details>
    </div></div>
    <div className="settings-section"><div className="settings-icon"><Icon name="terminal" /></div><div><h3>SSH 终端 <span className="feature-badge">独立连接</span></h3><p>密码认证、本机私钥认证、主机指纹核验、交互终端和窗口尺寸同步。每个进程只连接一台服务器。文字聊天不会写入终端；明确启动的终端任务通过受控命令卡片操作同一个 PTY。</p></div></div>
    <div className="settings-section"><div className="settings-icon"><Icon name="shield" /></div><div><h3>能力与边界 <span className="subtle-badge">明确执行</span></h3><p>文字聊天与终端任务分开。终端任务每次启动需要数据授权和 Shell 就绪确认；风险或不确定命令逐条批准。停止 AI 不会发送 Ctrl+C，命令写入后结果仍可能未知。手动终端始终可用，由你避免输入冲突。持久化历史不会恢复执行或重放批准；远端完成状态仍需人工核实。</p></div></div>
    <HistorySettings onStatus={onHistoryStatus} native={model.native} onRestore={chat => { model.restoreChat(chat); onClose(); }} />
    <div className="modal-footer"><button className="button primary" onClick={onClose}>关闭设置</button></div>
  </Modal>;
}
