import { useCallback, useEffect, useImperativeHandle, useLayoutEffect, useRef, useState, type MouseEvent, type ReactNode, type Ref } from 'react';
import { createPortal } from 'react-dom';
import { Check, Ellipsis } from 'lucide-react';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ErrorMark, useLongPress, useMenuKeyboard, type MapDeviceMenuItem } from '@pockymoe/thread-ui';
import './managed-object-menu.css';

/**
 * One action of a managed object (a device, a workspace). `deviceActions` and
 * `workspaceActions` return these in one order, so every surface that lists the
 * object offers the same menu. The map's device menu (shared UI) takes the same
 * items; it draws no icons.
 */
export interface ManagedAction extends MapDeviceMenuItem {
  icon?: ReactNode;
  /** A short note after the label, such as "Shared" on a device someone else owns. */
  detail?: ReactNode;
  /** One of a set of choices (the device switcher): a radio item, checked when current. */
  checked?: boolean;
}

export interface ManagedObjectMenuHandle {
  /** Open at a point of the page: a right click or a long press on the object's row. */
  openAt(x: number, y: number): void;
}

const MARGIN = 8;

function viewportBox() {
  const viewport = window.visualViewport;
  return { left: viewport?.offsetLeft ?? 0, top: viewport?.offsetTop ?? 0, width: viewport?.width ?? window.innerWidth, height: viewport?.height ?? window.innerHeight };
}

/**
 * The ⋯ menu of a managed object (docs/ui-principles.md §3): a horizontal-dots
 * trigger, a `role=menu` at the page root that follows its trigger and stays in
 * the visible viewport, the shared menu keyboard, and the same menu from a right
 * click or a long press on the object's row (`useManagedObjectMenu`).
 */
export function ManagedObjectMenu({ name, items, ref, className = '', trigger: custom }: {
  /** The object's name, for the trigger's and the menu's accessible names. */
  name: string;
  items: readonly ManagedAction[];
  ref?: Ref<ManagedObjectMenuHandle>;
  /** The trigger's look where it sits (a card's small key, a row's icon button). */
  className?: string;
  /** A trigger that shows what it switches (the device switcher) instead of ⋯; the menu opens under its start edge. */
  trigger?: { content: ReactNode; label: string; menuLabel: string } | undefined;
}) {
  useI18n();
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  // Where the menu hangs, as an offset from its trigger, so it follows the trigger when the page scrolls.
  const anchor = useRef({ x: 0, y: 0, alignEnd: true });
  const [position, setPosition] = useState<{ left: number; top: number } | null>(null);
  const close = useCallback((restore = false) => { setPosition(null); if (restore) trigger.current?.focus(); }, []);
  const place = useCallback(() => {
    const box = trigger.current?.getBoundingClientRect();
    if (!box) return null;
    const view = viewportBox();
    // The trigger left the visible page: the menu goes with it.
    if (box.bottom < view.top || box.top > view.top + view.height || box.right < view.left || box.left > view.left + view.width) return null;
    const size = menu.current?.getBoundingClientRect();
    const width = size?.width ?? 220, height = size?.height ?? items.length * 36 + 16;
    const x = box.left + anchor.current.x - (anchor.current.alignEnd ? width : 0);
    let y = box.top + anchor.current.y;
    // No room below the trigger: open above it instead of over it.
    if (anchor.current.alignEnd && y + height > view.top + view.height - MARGIN && box.top - 4 - height >= view.top + MARGIN) y = box.top - 4 - height;
    return {
      left: Math.max(view.left + MARGIN, Math.min(x, view.left + view.width - width - MARGIN)),
      top: Math.max(view.top + MARGIN, Math.min(y, view.top + view.height - height - MARGIN)),
    };
  }, [items.length]);
  const show = (x: number, y: number, alignEnd: boolean) => {
    const box = trigger.current!.getBoundingClientRect();
    anchor.current = { x: x - box.left, y: y - box.top, alignEnd };
    setPosition(place() ?? { left: x, top: y });
  };
  // ⋯ hangs its menu from its end edge; a switcher from its start edge, like a select.
  const openFromTrigger = () => { const box = trigger.current!.getBoundingClientRect(); if (custom) show(box.left, box.bottom + 4, false); else show(box.right, box.bottom + 4, true); };
  useImperativeHandle(ref, () => ({ openAt: (x, y) => show(x, y, false) }));
  useMenuKeyboard(menu, Boolean(position), close);
  const open = Boolean(position);
  // The first placement guesses the menu's size; measure it before the first paint.
  useLayoutEffect(() => {
    if (!open) return;
    const next = place();
    if (next) setPosition(current => current && (current.left !== next.left || current.top !== next.top) ? next : current);
  }, [open, place]);
  useEffect(() => {
    if (!open) return;
    const outside = (event: Event) => { if (!menu.current?.contains(event.target as Node) && !trigger.current?.contains(event.target as Node)) close(); };
    const reposition = () => { const next = place(); if (next) setPosition(next); else close(); };
    const scroll = (event: Event) => { if (!menu.current?.contains(event.target as Node)) reposition(); };
    document.addEventListener('pointerdown', outside);
    window.addEventListener('scroll', scroll, true); window.addEventListener('resize', reposition);
    window.visualViewport?.addEventListener('resize', reposition); window.visualViewport?.addEventListener('scroll', reposition);
    return () => {
      document.removeEventListener('pointerdown', outside);
      window.removeEventListener('scroll', scroll, true); window.removeEventListener('resize', reposition);
      window.visualViewport?.removeEventListener('resize', reposition); window.visualViewport?.removeEventListener('scroll', reposition);
    };
  }, [close, open, place]);
  return <>
    <button ref={trigger} type="button" className={`${custom ? 'managed-menu-switch' : 'managed-menu-trigger'} ${className}`} aria-label={custom?.label ?? translate('devices.moreActionsFor', { value1: name })} aria-haspopup="menu" aria-expanded={open}
      onClick={() => { if (open) { close(); return; } openFromTrigger(); }}
      onKeyDown={event => { if (!open && (event.key === 'ArrowDown' || event.key === 'ArrowUp')) { event.preventDefault(); openFromTrigger(); } }}>
      {custom ? custom.content : <Ellipsis data-icon="more" aria-hidden="true" />}
    </button>
    {position && createPortal(<div ref={menu} className="managed-menu" role="menu" aria-label={custom?.menuLabel ?? translate('devices.actionsFor', { value1: name })} data-theme-effective={document.documentElement.dataset.themeEffective} style={position}>
      {items.map(item => <div key={item.id} className="managed-menu-row" role="none" data-separated={item.separator || undefined}>
        {item.separator ? <span role="separator" className="sr-only" /> : null}
        <button type="button" role={item.checked === undefined ? 'menuitem' : 'menuitemradio'} aria-checked={item.checked} disabled={item.disabled} data-tone={item.tone} data-action={item.id} onClick={() => { close(true); item.onSelect(); }}>
          {item.icon ? <span className="managed-menu-icon" aria-hidden="true">{item.icon}</span> : null}
          <span className="managed-menu-label">{item.label}</span>
          {item.detail ? <span className="managed-menu-detail">{item.detail}</span> : null}
          {item.checked ? <Check className="managed-menu-check" aria-hidden="true" /> : null}
        </button>
        {/* An action this object cannot take now stays in its place, greyed, with the reason on a mark (§11). */}
        {item.disabledReason ? <ErrorMark kind="unavailable" message={item.disabledReason} /> : null}
      </div>)}
    </div>, document.body)}
  </>;
}

/**
 * The other two ways into the same menu (§3): spread `rowProps` on the object's
 * row for a right click and a long press, and pass `ref` to its ManagedObjectMenu.
 */
export function useManagedObjectMenu() {
  const ref = useRef<ManagedObjectMenuHandle>(null);
  const longPress = useLongPress(point => ref.current?.openAt(point.x, point.y));
  const rowProps = {
    ...longPress,
    onContextMenu(event: MouseEvent) {
      // Text being edited keeps the browser's own menu.
      if ((event.target as HTMLElement).closest?.('input, textarea, [contenteditable="true"]')) return;
      event.preventDefault();
      ref.current?.openAt(event.clientX, event.clientY);
    },
  };
  return { ref, rowProps };
}
