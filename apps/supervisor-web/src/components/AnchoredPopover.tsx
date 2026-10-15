import { translate } from '@pockymoe/thread-ui/i18n';
import { useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from 'react';
import { createPortal } from 'react-dom';
import { popoverPosition } from '../lib/popoverPosition';

/** A small panel below its trigger that stays inside the visible viewport. */
export function AnchoredPopover({ anchor, open, onClose, label, width = 320, children }: {
  anchor: RefObject<HTMLElement | null>;
  open: boolean;
  onClose: () => void;
  label: string;
  width?: number;
  children: ReactNode;
}) {
  const panel = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ top: 0, left: 12, maxWidth: width, maxHeight: 600 });
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const rect = anchor.current?.getBoundingClientRect();
      const box = panel.current?.getBoundingClientRect();
      const viewport = window.visualViewport;
      if (rect && box) setPosition(popoverPosition(rect, { width, height: box.height }, {
        width: viewport?.width ?? window.innerWidth,
        height: viewport?.height ?? window.innerHeight,
        left: viewport?.offsetLeft ?? 0,
        top: viewport?.offsetTop ?? 0,
      }));
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { onClose(); anchor.current?.focus(); }
    };
    place();
    window.addEventListener('resize', place);
    window.addEventListener('scroll', place, true);
    window.addEventListener('keydown', escape);
    const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(place);
    if (panel.current) observer?.observe(panel.current);
    return () => {
      window.removeEventListener('resize', place);
      window.removeEventListener('scroll', place, true);
      window.removeEventListener('keydown', escape);
      observer?.disconnect();
    };
  }, [open, width]);
  if (!open) return null;
  return createPortal(
    <>
      <button aria-label={translate('files.close')} className="fixed inset-0 z-[99] cursor-default" onClick={onClose} tabIndex={-1} type="button" />
      <div
        aria-label={label}
        className="anchored-popover fixed z-[100] space-y-3 overflow-y-auto rounded-xl border border-[var(--theme-border)] bg-[var(--theme-panel)] p-4 text-left text-sm shadow-xl"
        ref={panel}
        role="dialog"
        style={{ top: position.top, left: position.left, width: `min(${width}px, calc(100vw - 24px))`, maxHeight: position.maxHeight }}
      >
        {children}
      </div>
    </>,
    document.body,
  );
}
