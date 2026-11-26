import { useEffect, useRef, useState } from 'react';
import { confirmAsync, ErrorMark, type PromptAttachmentUpload } from '@pockymoe/thread-ui';
import { getLocale, translate } from '@pockymoe/thread-ui/i18n';
import { rewindPromptDraft, type RewindPrompt } from './rewindPrompt';
import { gitWrite } from '../../lib/gitApi';
import { FormDialog } from '../FormDialog';
export function RewindDialog({ threadId, turnId, deviceId, onClose, onRestored, onUndoAvailable, hasDraft, hasQueuedPrompts }: { threadId: string; turnId: string; deviceId?: string | null | undefined; hasDraft?: boolean; hasQueuedPrompts?: boolean; onUndoAvailable?: (undoId: string) => void; onClose(): void; onRestored(prompt: { text: string; attachments: PromptAttachmentUpload[] }): void }) {
  const zh = getLocale() === 'zh-CN';
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [restoreReason, setRestoreReason] = useState<string | null>('…');
  const [previewRevision, setPreviewRevision] = useState(0);
  const previewRequest = useRef<{ key: string; promise: Promise<unknown> } | null>(null);
  useEffect(() => {
    let active = true;
    const key = JSON.stringify([threadId, turnId, deviceId, previewRevision]);
    // StrictMode replays effects; reuse the in-flight preview so it cannot conflict with itself.
    if (previewRequest.current?.key !== key) previewRequest.current = { key, promise: gitWrite(threadId, 'checkpoints/preview', { turnId, phase: 'start' }, deviceId) };
    void previewRequest.current.promise.then(() => { if (active) setRestoreReason(null); }).catch(caught => { if (active) setRestoreReason(String(caught)); });
    return () => { active = false; };
  }, [threadId, turnId, deviceId, previewRevision]);
  const [dropQueued, setDropQueued] = useState(false);
  const [restoreFiles, setRestoreFiles] = useState(false);
  const rewind = async () => {
    if (hasDraft && !await confirmAsync({ title: zh ? '替换当前草稿？' : 'Replace current draft?', description: zh ? '回退的提示词将替换当前草稿。' : 'The rewound prompt will replace the current draft.', confirmLabel: zh ? '替换' : 'Replace' })) return;
    setBusy(true); setError('');
    try {
      const result = await gitWrite<{ prompt: RewindPrompt; undoId?: string }>(threadId, 'rewind', { turnId, restoreFiles, ...(dropQueued ? { dropQueued: true } : {}) }, deviceId);
      if (result.undoId) onUndoAvailable?.(result.undoId);
      // Preserve the durable prompt immediately even if an attachment download fails.
      onRestored({ text: result.prompt.text, attachments: [] });
      if (result.prompt.attachments.length) onRestored(await rewindPromptDraft(threadId, result.prompt, deviceId));
      onClose();
    }
    catch (caught) { setError(String(caught)); } finally { setBusy(false); }
  };
  return <FormDialog secondary className="thread-git-rewind" title={zh ? '从这里编辑' : 'Edit from here'} busy={busy} onClose={onClose}>
    <p>{zh ? '推送、消息和委派等外部操作不会撤销。' : 'External effects such as pushes, messages and delegates remain.'}</p>
    <label><input type="radio" name="rewind" checked={!restoreFiles} onChange={() => setRestoreFiles(false)} />{zh ? '回退对话记录' : 'Rewind conversation'}</label>
    <label><input type="radio" name="rewind" disabled={restoreReason !== null} checked={restoreFiles} onChange={() => setRestoreFiles(true)} />{zh ? '回退对话记录和文件' : 'Rewind conversation and files'}</label>
    {hasQueuedPrompts && <label><input type="checkbox" checked={dropQueued} onChange={event => setDropQueued(event.target.checked)} />{zh ? '丢弃排队的消息' : 'Discard queued prompts'}</label>}
    {restoreReason && <p role="status">{restoreReason}{restoreReason !== '…' && <button onClick={() => { setRestoreReason('…'); setPreviewRevision(value => value + 1); }}>{translate('workbench.retry')}</button>}</p>}
    {error && <ErrorMark message={error} />}
    <div className="form-dialog-actions"><button disabled={busy} onClick={onClose}>{zh ? '取消' : 'Cancel'}</button><button className="relay-button-primary" disabled={busy || hasQueuedPrompts && !dropQueued} onClick={() => { void rewind(); }}>{zh ? '回退' : 'Rewind'}</button></div>
  </FormDialog>;
}
