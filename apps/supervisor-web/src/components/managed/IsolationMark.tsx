import { useRef, useState } from 'react';
import { Lock } from 'lucide-react';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { AnchoredPopover } from '../AnchoredPopover';

/**
 * "Isolated", the same on a device and on a workspace: a lock with the word.
 * Tapping or clicking it says what isolation keeps out, so the explanation is
 * not left to a hover title (docs/ui-principles.md §8).
 */
export function IsolationMark({ kind }: { kind: 'device' | 'workspace' }) {
  useI18n();
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const label = translate(kind === 'device' ? 'devices.deviceIsolated' : 'files.workspaceIsolated');
  return <>
    <button ref={trigger} type="button" className="isolation-mark" aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen(!open)}>
      <Lock data-icon="isolated" aria-hidden="true" className="h-3 w-3" />
      <span className="isolation-mark-text">{label}</span>
    </button>
    <AnchoredPopover anchor={trigger} label={label} onClose={() => setOpen(false)} open={open} width={300}>
      <p className="text-[var(--theme-fg)]">{translate(kind === 'device' ? 'devices.deviceIsolationDescription' : 'files.workspaceIsolatedHint')}</p>
    </AnchoredPopover>
  </>;
}
