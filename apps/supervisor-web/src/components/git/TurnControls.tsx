import { useState } from 'react';
import { confirmAsync, PathChip, Select } from '@pockymoe/thread-ui';
import { gitWrite, type TurnCheckpoint } from '../../lib/gitApi';
import { gitText as gt } from '../../lib/gitI18n';
import { parsePatch } from './patch';
import type { Write } from './GitActions';
export function TurnControls({ turns, turnId, onTurn, workingCopyId, threadId, deviceId, write, busy }: { turns: TurnCheckpoint[]; turnId: string; onTurn(id: string): void; workingCopyId: string; threadId: string; deviceId?: string | null | undefined; write: Write; busy: boolean }) {
  const [preview, setPreview] = useState<{ phase: string; files: string[] } | null>(null);
  const [undoId, setUndoId] = useState('');
  const [error, setError] = useState('');
  const selected = turns.find(turn => turn.turnId === turnId)?.workingCopies.find(copy => copy.workingCopyId === workingCopyId);
  const restore = async (phase: 'start' | 'end') => {
    setError('');
    try {
      const value = await gitWrite<{ files?: (string | { path: string })[]; patch?: string }>(threadId, 'checkpoints/preview', { turnId, phase, workingCopyId }, deviceId);
      const files = (value.files ?? parsePatch(value.patch ?? '').files).map(file => typeof file === 'string' ? file : file.path);
      setPreview({ phase, files });
      if (!await confirmAsync({ title: gt('restoreConfirm'), description: files.join('\n') || gt('noFiles'), confirmLabel: gt(phase === 'start' ? 'restoreBefore' : 'restoreAfter') })) return;
      const result = await write('checkpoints/restore', { turnId, phase }) as { undoId?: string } | undefined;
      if (result?.undoId) setUndoId(result.undoId);
    } catch (caught) { setError(String(caught)); }
  };
  return <div className="thread-git-turn-controls">
    {turns.length ? <Select aria-label={gt('turn')} value={turnId} onValueChange={onTurn} options={turns.map(turn => { const counts = turn.workingCopies.find(copy => copy.workingCopyId === workingCopyId); return { value: turn.turnId, label: `${gt('turn')} ${turn.turnIndex} · ${new Date(turn.startedAt).toLocaleString()} · ${counts?.files ?? 0} +${counts?.additions ?? 0} −${counts?.deletions ?? 0}` }; })} /> : <p>{gt('noCheckpoints')}</p>}
    <div className="thread-git-actions">{(['start', 'end'] as const).map(phase => <button key={phase} disabled={busy || !selected?.available} title={selected?.reason} onClick={() => { void restore(phase); }}>{gt(phase === 'start' ? 'restoreBefore' : 'restoreAfter')}</button>)}</div>
    {selected && !selected.available && <p role="status">{selected.reason}</p>}
    {preview && <details><summary>{gt('restoreConfirm')}</summary>{preview.files.map(path => <PathChip key={path} path={path} />)}</details>}
    {undoId && <div role="status">{gt('restored')} <button disabled={busy} onClick={() => { void write('checkpoints/undo', { undoId }).then(result => { if (result) setUndoId(''); }).catch(() => {}); }}>{gt('undo')}</button></div>}
    {error && <p role="alert">{error}</p>}
  </div>;
}
