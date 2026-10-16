import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useRef, useState } from 'react';
import type { ConnectionProtocol, ConnectionSummary } from '../lib/connections';
import { ConnectionForm } from './ConnectionForm';
import { FormDialog } from './FormDialog';

/**
 * Add or edit a connection over whatever opened it. Unsaved edits need an explicit
 * discard, and focus returns to the control that opened the dialog.
 */
export function ConnectionDialog({
  apiRoot,
  connection,
  protocols,
  onSaved,
  onClose,
}: {
  apiRoot: string;
  connection?: ConnectionSummary | null;
  protocols?: ConnectionProtocol[];
  onSaved: (connection: ConnectionSummary) => void;
  onClose: () => void;
}) {
  useI18n();
  const [dirty, setDirty] = useState(false);
  const [confirmDiscard, setConfirmDiscard] = useState(false);
  const opener = useRef<Element | null>(
    typeof document === 'undefined' ? null : document.activeElement,
  );
  useEffect(() => {
    const element = opener.current;
    return () => {
      // Radix restores focus too; this covers openers inside menus that re-rendered.
      window.setTimeout(() => {
        if (
          element instanceof HTMLElement &&
          element.isConnected &&
          (!document.activeElement || document.activeElement === document.body)
        )
          element.focus();
      });
    };
  }, []);
  function requestClose() {
    if (dirty) setConfirmDiscard(true);
    else onClose();
  }
  return (
    <FormDialog
      title={translate(
        connection?.id ? 'settings.connEditTitle' : 'settings.connAddTitle',
      )}
      onClose={requestClose}
    >
      {confirmDiscard && (
        <div
          role="alertdialog"
          aria-label={translate('settings.connDiscardTitle')}
          className="rounded-lg border border-[var(--theme-border)] bg-[var(--theme-hover)] p-3 text-sm"
        >
          <p>{translate('settings.connDiscardHint')}</p>
          <div className="mt-2 flex flex-wrap gap-2">
            <button
              type="button"
              autoFocus
              className="host-secondary-button min-h-9 rounded-lg border px-3 text-xs"
              onClick={() => setConfirmDiscard(false)}
            >
              {translate('settings.connKeepEditing')}
            </button>
            <button
              type="button"
              className="min-h-9 rounded-lg border border-[var(--theme-border)] px-3 text-xs text-[var(--status-danger-fg)]"
              onClick={onClose}
            >
              {translate('settings.connDiscard')}
            </button>
          </div>
        </div>
      )}
      <div hidden={confirmDiscard}>
        <ConnectionForm
          apiRoot={apiRoot}
          initial={connection ?? null}
          {...(protocols ? { protocols } : {})}
          onDirtyChange={setDirty}
          onCancel={requestClose}
          onSaved={onSaved}
        />
      </div>
    </FormDialog>
  );
}
