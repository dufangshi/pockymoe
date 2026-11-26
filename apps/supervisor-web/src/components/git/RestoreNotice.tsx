import { useState } from 'react';
import { ErrorMark } from '@pockymoe/thread-ui';
import { gitWrite } from '../../lib/gitApi';
import { gitText as gt } from '../../lib/gitI18n';
export function RestoreNotice({ threadId, undoId, deviceId, onClose }: { threadId: string; undoId: string; deviceId?: string | null | undefined; onClose(): void }) {
  const [busy, setBusy] = useState(false), [error, setError] = useState('');
  return <div className="thread-git-restore-notice thread-git-actions" role="status"><span>{gt('restored')}</span><button disabled={busy} onClick={() => { setBusy(true); void gitWrite(threadId, 'checkpoints/undo', { undoId }, deviceId).then(onClose).catch(caught => setError(String(caught))).finally(() => setBusy(false)); }}>{gt('undo')}</button><button aria-label={gt('closeNotice')} disabled={busy} onClick={onClose}>×</button>{error && <ErrorMark message={error} />}</div>;
}
