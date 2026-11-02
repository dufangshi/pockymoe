/** Reuse the focused overlay's Escape handling, including busy/unsaved guards. */
export function handleMobileBack(navigate: (delta: number) => void): boolean {
  const overlays = Array.from(document.querySelectorAll<HTMLElement>('[role="menu"], [role="dialog"], [role="alertdialog"], .error-mark-bubble, #app-shell-navigation-menu')).filter(node => node.getClientRects().length && !node.closest('[hidden], [inert]'));
  const overlay = overlays.at(-1);
  if (overlay) {
    if (!overlay.contains(document.activeElement)) (overlay.querySelector<HTMLElement>('button, [tabindex]') ?? overlay).focus();
    (document.activeElement ?? overlay).dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
    return true;
  }
  // React Router's index counts only entries belonging to this WebView, so a
  // cold notification deep link cannot return to an unrelated external page.
  if (Number(window.history.state?.idx ?? 0) > 0) { navigate(-1); return true; }
  return false;
}
