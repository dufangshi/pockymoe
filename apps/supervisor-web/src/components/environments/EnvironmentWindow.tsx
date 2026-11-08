import { useEffect, useState, type ReactNode } from 'react';
import { Dialog, DialogContent, DialogTitle } from '@pockymoe/thread-ui';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { environmentText as t } from '../../lib/environmentI18n';
import './environments.css';
import { EnvironmentConfirmation } from './EnvironmentConfirmation';

type Slot = ReactNode | ((close: () => void) => ReactNode);
const render = (slot: Slot, close: () => void) => typeof slot === 'function' ? slot(close) : slot;

/** Shared Dialog supplies portal/focus handling, including nested Settings windows.
    Layout: title (and an optional subtitle) with header tools, a scrolling body,
    and a fixed footer for the window's decision (UI §4). */
export function EnvironmentWindow({ title, subtitle, tools, children, footer, wide = false, onClose, dirty = false, busy = false }: {
  title: ReactNode; subtitle?: ReactNode; tools?: ReactNode; children: Slot; footer?: Slot; wide?: boolean;
  onClose(): void; dirty?: boolean; busy?: boolean;
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
      <DialogContent className={`host-dialog environment-window host-secondary-window${wide ? ' is-wide' : ''}`} overlayClassName="environment-overlay host-secondary-window-overlay"
        onCloseAutoFocus={event => { event.preventDefault(); const target = trigger?.getClientRects().length ? trigger : threadTools; if (target?.isConnected) target.focus({ preventScroll: true }); }}
        aria-describedby={undefined} onEscapeKeyDown={event => { event.preventDefault(); close(); }}
        onPointerDownOutside={event => { event.preventDefault(); if (!discard) close(); }}>
        <header className="environment-window-header">
          <div className="environment-window-titles"><DialogTitle>{title}</DialogTitle>{subtitle && <div className="environment-window-subtitle">{subtitle}</div>}</div>
          {tools && <div className="environment-window-tools">{tools}</div>}
        </header>
        <div className="host-secondary-window-body environment-window-body">{render(children, close)}</div>
        {footer && <footer className="environment-window-footer">{render(footer, close)}</footer>}
      </DialogContent>
    </Dialog>
    <EnvironmentConfirmation open={discard} title={t('discardTitle')} description={t('discardBody')} confirmLabel={t('discard')}
      onCancel={() => setDiscard(false)} onConfirm={onClose} />
  </>;
}
