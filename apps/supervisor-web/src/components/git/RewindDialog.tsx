import { useEffect, useState } from 'react';
import { ErrorMark, type PromptAttachmentUpload } from '@pockymoe/thread-ui';
import { getLocale } from '@pockymoe/thread-ui/i18n';
import { downloadLinkedFile } from '../../lib/api';
import { gitWrite } from '../../lib/gitApi';
import { FormDialog } from '../FormDialog';
export function RewindDialog({ threadId, turnId, deviceId, onClose, onRestored }: { threadId: string; turnId: string; deviceId?: string | null | undefined; onClose(): void; onRestored(prompt: { text: string; attachments: PromptAttachmentUpload[] }): void }) {
  const zh = getLocale() === 'zh-CN';
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [restoreReason, setRestoreReason] = useState<string | null>('…');
  useEffect(() => {
    let active = true;
    void gitWrite(threadId, 'checkpoints/preview', { turnId, phase: 'start' }, deviceId).then(() => { if (active) setRestoreReason(null); }).catch(caught => { if (active) setRestoreReason(String(caught)); });
    return () => { active = false; };
  }, [threadId, turnId, deviceId]);
  const [restoreFiles, setRestoreFiles] = useState(false);
  const rewind = async () => {
    setBusy(true); setError('');
    try { const result = await gitWrite<{ prompt: { text: string; attachments: (Omit<PromptAttachmentUpload, 'file'> & { path: string })[] } }>(threadId, 'rewind', { turnId, restoreFiles }, deviceId); const attachments = await Promise.all(result.prompt.attachments.map(async attachment => { const { blob } = await downloadLinkedFile(threadId, attachment.path, deviceId); return { ...attachment, file: new File([blob], attachment.originalName, { type: blob.type }) }; })); onRestored({ ...result.prompt, attachments }); onClose(); }
    catch (caught) { setError(String(caught)); } finally { setBusy(false); }
  };
  return <FormDialog title={zh ? '从这里编辑' : 'Edit from here'} busy={busy} onClose={onClose}>
    <p>{zh ? '推送、消息和委派等外部操作不会撤销。' : 'External effects such as pushes, messages and delegates remain.'}</p>
    <label><input type="radio" name="rewind" checked={!restoreFiles} onChange={() => setRestoreFiles(false)} />{zh ? '回退对话记录' : 'Rewind conversation'}</label>
    <label><input type="radio" name="rewind" disabled={restoreReason !== null} checked={restoreFiles} onChange={() => setRestoreFiles(true)} />{zh ? '回退对话记录和文件' : 'Rewind conversation and files'}</label>
    {restoreReason && <p role="status">{restoreReason}</p>}
    {error && <ErrorMark message={error} />}
    <div className="form-dialog-actions"><button disabled={busy} onClick={onClose}>{zh ? '取消' : 'Cancel'}</button><button disabled={busy} onClick={() => { void rewind(); }}>{zh ? '回退' : 'Rewind'}</button></div>
  </FormDialog>;
}
