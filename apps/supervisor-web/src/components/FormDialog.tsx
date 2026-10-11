import type { ReactNode } from 'react';
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
}: {
  title: string;
  description?: string;
  /** Small inline details shown beside the title. */
  meta?: ReactNode;
  className?: string;
  children: ReactNode;
  onClose: () => void;
  busy?: boolean;
}) {
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !busy) onClose();
      }}
    >
      <DialogContent
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
