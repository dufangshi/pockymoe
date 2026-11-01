import type { KeyboardEvent } from 'react';
export function menuKeyboard(event: KeyboardEvent<HTMLElement>, close: () => void) {
  if (event.key === 'Escape') { event.preventDefault(); close(); return; }
  if (!['ArrowDown', 'ArrowUp', 'Home', 'End', 'Tab'].includes(event.key)) return;
  const items = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'));
  const current = items.indexOf(document.activeElement as HTMLButtonElement);
  const direction = event.key === 'ArrowUp' || (event.key === 'Tab' && event.shiftKey) ? -1 : 1;
  const next = event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1 : (current + direction + items.length) % items.length;
  event.preventDefault(); items[next]?.focus();
}
