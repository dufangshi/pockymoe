import { useEffect, useState, type ReactNode } from 'react';
import { Dialog, DialogContent, DialogTitle } from '@pockymoe/thread-ui';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { environmentText as t } from '../../lib/environmentI18n';
import './environments.css';
import { EnvironmentConfirmation } from './EnvironmentConfirmation';

/** Shared Dialog supplies portal/focus handling, including nested Settings windows. */
export function EnvironmentWindow({ title, children, onClose, dirty = false, busy = false }: {
  title: string; children: ReactNode | ((close: () => void) => ReactNode); onClose(): void; dirty?: boolean; busy?: boolean;
}) {
  useI18n();
  const [discard, setDiscard] = useState(false);
  const [trigger] = useState(() => document.activeElement instanceof HTMLElement ? document.activeElement : null);
  const [threadTools] = useState(() => trigger?.closest('#matter-thread-tools') ? document.querySelector<HTMLElement>('[aria-controls="matter-thread-tools"]') : null);
  const close = () => { if (!busy) { if (dirty) setDiscard(true); else onClose(); } };
  useEffect(() => {
    if (!dirty) return;
    const leave = (event: BeforeUnloadEvent) => { event.preventDefault(); event.returnValue = ''; };
    window.addEventListener('beforeunload', leave);
    return () => window.removeEventListener('beforeunload', leave);
  }, [dirty]);
  return <>
    <Dialog open onOpenChange={open => { if (!open) close(); }}>
      <DialogContent className="host-dialog environment-window host-secondary-window" overlayClassName="environment-overlay host-secondary-window-overlay"
        onCloseAutoFocus={event => { event.preventDefault(); const target = trigger?.getClientRects().length ? trigger : threadTools; if (target?.isConnected) target.focus({ preventScroll: true }); }}
        aria-describedby={undefined} onEscapeKeyDown={event => { event.preventDefault(); close(); }}
        onPointerDownOutside={event => { event.preventDefault(); if (!discard) close(); }}>
        <DialogTitle>{title}</DialogTitle>
        <div className="host-secondary-window-body environment-window-body">{typeof children === 'function' ? children(close) : children}</div>
      </DialogContent>
    </Dialog>
    <EnvironmentConfirmation open={discard} title={t('discardTitle')} description={t('discardBody')} confirmLabel={t('discard')}
      onCancel={() => setDiscard(false)} onConfirm={onClose} />
  </>;
}
