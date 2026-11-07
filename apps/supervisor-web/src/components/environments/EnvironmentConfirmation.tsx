import type { ComponentProps } from 'react';
import { ConfirmDialog, Dialog, DialogContent, DialogTitle } from '@pockymoe/thread-ui';

/** Register the shared confirmation as a nested focus scope. Settings uses Radix;
 * pausing its focus trap lets ConfirmDialog's useModalFocus own the top window. */
export function EnvironmentConfirmation(props: ComponentProps<typeof ConfirmDialog>) {
  if (!props.open) return null;
  return <Dialog open modal={false}>
    <DialogContent role="presentation" className="environment-confirmation-focus" style={{ display: 'contents' }} showCloseButton={false} aria-describedby={undefined}
      onOpenAutoFocus={event => event.preventDefault()} onCloseAutoFocus={event => event.preventDefault()}
      onInteractOutside={event => event.preventDefault()} onEscapeKeyDown={event => event.preventDefault()}>
      <DialogTitle className="sr-only">{props.title}</DialogTitle><ConfirmDialog {...props} />
    </DialogContent>
  </Dialog>;
}
