import { useRef, type ReactNode } from 'react';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from '@pockymoe/thread-ui';

export function FormDialog({
  title,
  description,
  children,
  onClose,
  busy = false,
  meta,
  className,
  focusCancel = false,
}: {
  title: string;
  description?: string;
  /** Small inline details shown beside the title. */
  meta?: ReactNode;
  className?: string;
  /** Destructive confirmation forms start on their explicit Cancel control. */
  focusCancel?: boolean;
  children: ReactNode;
  onClose: () => void;
  busy?: boolean;
}) {
  const content = useRef<HTMLDivElement>(null);
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !busy) onClose();
      }}
    >
      <DialogContent
        ref={content}
        onOpenAutoFocus={event => {
          if (focusCancel) { event.preventDefault(); content.current?.querySelector<HTMLButtonElement>('[data-dialog-cancel]')?.focus(); }
        }}
        className={`host-dialog ${className ?? ''} !z-[130] !max-h-[calc(100dvh-2rem)] !overflow-y-auto !overscroll-contain`}
        overlayClassName="!z-[129]"
        {...(!description ? { 'aria-describedby': undefined } : {})}
        onPointerDownOutside={(event) => {
          if (busy) event.preventDefault();
        }}
        onEscapeKeyDown={(event) => {
          if (busy) event.preventDefault();
        }}
      >
        <DialogHeader className={meta ? 'text-left' : undefined}>
          {meta ? (
            <div className="form-dialog-title-row">
              <DialogTitle>{title}</DialogTitle>
              {meta}
            </div>
          ) : (
            <DialogTitle>{title}</DialogTitle>
          )}
          {description && <DialogDescription>{description}</DialogDescription>}
        </DialogHeader>
        {children}
      </DialogContent>
    </Dialog>
  );
}
