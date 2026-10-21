import { ErrorMark } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { FormEvent, useId, useRef } from 'react';
import { createPortal } from 'react-dom';
import { X } from 'lucide-react';

import { useDialogLifecycle } from './useDialogLifecycle';

interface RenameDialogProps {
  open: boolean;
  title: string;
  label: string;
  value: string;
  busy?: boolean;
  error?: string | null;
  onChange: (value: string) => void;
  onCancel: () => void;
  onSubmit: () => void | Promise<void>;
}

export function RenameDialog({
  open,
  title,
  label,
  value,
  busy = false,
  error,
  onChange,
  onCancel,
  onSubmit,
}: RenameDialogProps) {
  useI18n();
  const dialogRef = useRef<HTMLFormElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const titleId = useId();
  const inputErrorId = useId();

  useDialogLifecycle({
    busy,
    containerRef: dialogRef,
    initialFocusRef: inputRef,
    onClose: onCancel,
    open,
  });

  if (!open) {
    return null;
  }

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    void onSubmit();
  }

  return createPortal(
    <div className="fixed inset-0 z-[95] flex items-center justify-center p-4 sm:p-6">
      <button
        type="button"
        aria-label={translate("workbench.closeRenameDialog")}
        onClick={onCancel}
        disabled={busy}
        className="ui-overlay-scrim absolute inset-0 backdrop-blur-sm disabled:cursor-not-allowed"
      />
      <form
        aria-labelledby={titleId}
        aria-modal="true"
        className="host-dialog relative z-[1] w-full max-w-md rounded-lg border p-5 shadow-[var(--theme-shadow)] sm:p-6"
        onSubmit={handleSubmit}
        ref={dialogRef}
        role="dialog"
        tabIndex={-1}
      >
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0 flex-1">
            <h2 className="host-page-title text-base font-semibold" id={titleId}>{title}</h2>
            <p className="host-muted mt-1 text-sm">
              {translate("workbench.changesAreSavedOnlyAfterConfirmation")}</p>
          </div>
          <button
            type="button"
            aria-label={translate("workbench.closeDialog")}
            onClick={onCancel}
            disabled={busy}
            className="host-icon-button inline-flex h-11 w-11 shrink-0 items-center justify-center rounded-md border transition disabled:cursor-not-allowed disabled:opacity-60 sm:h-9 sm:w-9"
          >
            <X aria-hidden="true" className="h-4 w-4" />
          </button>
        </div>

        <div className="mt-5">
          <label htmlFor="rename-dialog-input" className="host-form-label text-sm font-medium">
            {label}
          </label>
          <input
            id="rename-dialog-input"
            aria-label={label}
            aria-describedby={error ? inputErrorId : undefined}
            aria-invalid={error ? true : undefined}
            disabled={busy}
            ref={inputRef}
            value={value}
            onChange={(event) => onChange(event.target.value)}
            className="host-form-control mt-2 w-full rounded-md border px-4 py-3 outline-none transition disabled:cursor-wait disabled:opacity-60"
          />
        </div>

        <div className="mt-5 flex flex-col-reverse gap-2 sm:flex-row sm:items-center sm:justify-end">
          <button
            type="button"
            onClick={onCancel}
            disabled={busy}
            className="host-secondary-button min-h-11 rounded-md border px-4 text-sm font-medium transition disabled:cursor-not-allowed disabled:opacity-60"
          >
            {translate("workbench.cancel")}</button>
          <div className="flex items-center gap-2">
            <button
              type="submit"
              disabled={busy || !value.trim()}
              className="ui-action-primary min-h-11 flex-1 rounded-md px-4 text-sm font-semibold transition disabled:cursor-not-allowed sm:flex-none"
            >
              {translate("workbench.save")}</button>
            {error ? <span className="contents" id={inputErrorId}><ErrorMark message={error} /></span> : null}
          </div>
        </div>
      </form>
    </div>,
    document.body,
  );
}
