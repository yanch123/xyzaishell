import type { TerminalAttachmentDraft } from '../lib/terminalSelection';
import { attachmentConsented, MAX_SELECTION_BYTES } from '../lib/terminalSelection';
import type { ModelConfigStatus } from '../lib/model';
import { utf8Length } from '../lib/chats';

export function TerminalAttachment({ attachment, config, stale, disabled, onEdit, onRemove, onConsent }: {
  attachment: TerminalAttachmentDraft; config: ModelConfigStatus; stale: boolean; disabled: boolean;
  onEdit: (text: string) => void; onRemove: () => void; onConsent: (value: boolean) => void;
}) {
  return <section className="terminal-attachment" aria-label="终端选区附件草稿">
    <div className="attachment-heading"><strong>终端选区 · 未发送</strong><button className="button secondary" onClick={onRemove} disabled={disabled}>移除附件</button></div>
    <p className="attachment-source">固定来源：{attachment.source.username}@{attachment.source.host}:{attachment.source.port} · 会话 {attachment.source.session_id} / {attachment.source.generation}</p>
    <p>{attachment.edited ? '已由你编辑，发送时会标记为编辑后的副本。' : '仅包含所选文本，来源不会随编辑或切换服务器而改变。'}{attachment.truncated && ' 选区已截断，仅保留前 8 KiB / 200 行。'}</p>
    <textarea aria-label="审阅并编辑终端选区" value={attachment.text} disabled={disabled || stale} maxLength={MAX_SELECTION_BYTES} onChange={event => onEdit(event.target.value)} spellCheck={false} />
    <div className="attachment-count">{utf8Length(attachment.text)} / {MAX_SELECTION_BYTES} B</div>
    <p>本次接收方：{config.destination ?? '尚未配置'} · 模型：{config.model ?? '尚未配置'}。服务可能记录内容或收费。请先移除密码、令牌和其他敏感信息。</p>
    {stale && <p role="alert" className="inline-error">附件来源已不匹配当前连接或聊天。不能发送；请移除并从当前会话重新选择。</p>}
    <label className="check-field"><input type="checkbox" checked={!stale && attachmentConsented(attachment, config)} disabled={disabled || stale || !config.ready || !attachment.text.trim()} onChange={event => onConsent(event.target.checked)} /><span>我已审阅上述选区及来源，同意将此附件、本次消息及当前聊天历史发送给此接收方，仅用于文字分析。</span></label>
  </section>;
}
