const RELOADED_AT = 'pockymoe.stale-build-reload-at';

/**
 * Each deployment replaces the hashed build chunks, so a page opened before it
 * fails to load the next lazy chunk (for example the Explorer). Reload once to
 * fetch the new build; a second failure within a minute is left to the error
 * boundary instead of reloading in a loop.
 */
export function installStaleBuildReload(win: Window = window, now = () => Date.now()) {
  win.addEventListener('vite:preloadError', event => {
    let last = 0;
    try { last = Number(win.sessionStorage.getItem(RELOADED_AT) ?? 0); } catch { /* storage may be unavailable */ }
    if (now() - last < 60_000) return;
    try { win.sessionStorage.setItem(RELOADED_AT, String(now())); } catch { return; }
    event.preventDefault();
    win.location.reload();
  });
}
