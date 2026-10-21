/** Menus, lists, sheets and windows that open over the page. */
const POPUP = [
  '[role="menu"]', '[role="listbox"]', '[role="dialog"]', '[data-composer-menu-surface]',
  '[data-radix-popper-content-wrapper]', '.recent-thread-menu', '.workspace-node-menu',
].join(', ');
/** What a finger presses in one of them. */
const CONTROL = [
  'button', 'a[href]', '[role="menuitem"]', '[role="menuitemradio"]', '[role="menuitemcheckbox"]',
  '[role="option"]', '[role="tab"]',
].join(', ');

function editing(element: Element | null) {
  return element instanceof HTMLElement && (element.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(element.tagName));
}

/**
 * A tap on iOS (every iOS browser is WebKit) goes through popups in two ways,
 * both handled here for touch only:
 *
 * - A pressed button never takes focus there, so focus leaves an open menu with
 *   no related target and a menu that closes on blur closes before the click.
 *   A press on a control in a popup focuses it, as other browsers do (not while
 *   the reader types, so the keyboard stays).
 * - The click is made after the finger lifts. A popup that closed on the press
 *   or the release (an item that acts on pointerup, an outside press) is gone,
 *   and the click lands on whatever was under it. A click whose pressed element
 *   left the page and that landed elsewhere is dropped.
 */
export function installTouchTapGuard(target: Document = document, { trustedOnly = true } = {}) {
  let press: { element: Element; at: number } | null = null;
  target.addEventListener('pointerdown', (event) => {
    const element = event.target instanceof Element ? event.target : null;
    press = element && (event.pointerType === 'touch' || event.pointerType === 'pen')
      ? { element, at: performance.now() }
      : null;
    if (!press || !element || editing(target.activeElement)) return;
    const control = element.closest(CONTROL);
    if (control instanceof HTMLElement && control !== target.activeElement && control.closest(POPUP)) {
      control.focus({ preventScroll: true });
    }
  }, true);
  target.addEventListener('click', (event) => {
    const pressed = press;
    press = null;
    if (!pressed || (trustedOnly && !event.isTrusted) || performance.now() - pressed.at > 1500) return;
    const landed = event.target instanceof Node ? event.target : null;
    if (!landed || pressed.element.isConnected || pressed.element.contains(landed) || landed.contains(pressed.element)) return;
    event.preventDefault();
    event.stopImmediatePropagation();
  }, true);
}
