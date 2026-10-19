import { useEffect, useRef, type RefObject } from 'react';

const focusableSelector = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

export function useDialogLifecycle({
  busy = false,
  containerRef,
  initialFocusRef,
  onClose,
  open,
}: {
  busy?: boolean;
  containerRef: RefObject<HTMLElement | null>;
  initialFocusRef?: RefObject<HTMLElement | null>;
  onClose: () => void;
  open: boolean;
}) {
  const busyRef = useRef(busy);
  const onCloseRef = useRef(onClose);
  busyRef.current = busy;
  onCloseRef.current = onClose;

  useEffect(() => {
    if (!open) {
      return;
    }

    const previousFocus = document.activeElement;
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = 'hidden';

    const focusTimer = window.setTimeout(() => {
      const target =
        initialFocusRef?.current ??
        containerRef.current?.querySelector<HTMLElement>(focusableSelector);
      target?.focus();
    }, 0);

    function handleKeyDown(event: KeyboardEvent) {
      const focusedDialog = document.activeElement?.closest('[role="dialog"]');
      if (focusedDialog && focusedDialog !== containerRef.current && !containerRef.current?.contains(focusedDialog)) return;

      if (event.key === 'Escape') {
        event.preventDefault();
        // Only the top window closes, not a window it was opened from; a busy
        // one stays, and so does the window under it.
        event.stopPropagation();
        if (!busyRef.current) onCloseRef.current();
        return;
      }

      if (event.key !== 'Tab') {
        return;
      }

      const focusable = Array.from(
        containerRef.current?.querySelectorAll<HTMLElement>(focusableSelector) ?? [],
      ).filter((element) => element.offsetParent !== null);
      if (focusable.length === 0) {
        event.preventDefault();
        containerRef.current?.focus();
        return;
      }

      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (!first || !last) {
        return;
      }
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }

    // Capture on window runs before the document listeners of an enclosing
    // modal window (Radix closes on Escape there).
    window.addEventListener('keydown', handleKeyDown, true);
    return () => {
      window.clearTimeout(focusTimer);
      window.removeEventListener('keydown', handleKeyDown, true);
      document.body.style.overflow = previousOverflow;
      window.setTimeout(() => {
        if (previousFocus instanceof HTMLElement && previousFocus.isConnected) {
          previousFocus.focus();
          return;
        }

        const fallback = document.querySelector<HTMLElement>(
          '[data-dialog-focus-fallback], main h1',
        );
        if (!fallback) {
          return;
        }
        if (!fallback.hasAttribute('tabindex')) {
          fallback.setAttribute('tabindex', '-1');
        }
        fallback.focus();
      }, 0);
    };
  }, [containerRef, initialFocusRef, open]);
}
