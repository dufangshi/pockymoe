// Resolved once, before the router or transport starts. The installed index has
// a relative <base> adjusted by the SPA server; a proxy may inject its own base.
export function resolveWebBase(baseURI: string, pageURL: string): string {
  const page = new URL(pageURL);
  const base = new URL('.', new URL(baseURI, page));
  if (base.origin !== page.origin) throw new Error('The Web base must be on the page origin');
  return base.pathname;
}

export const webBase = import.meta.env.VITE_POCKYMOE_STATIC_WEB === 'true' ? '/' : typeof document === 'undefined'
  ? (typeof self !== 'undefined' && 'registration' in self
      ? new URL((self as unknown as { registration: { scope: string } }).registration.scope).pathname : '/')
  : resolveWebBase(document.baseURI, window.location.href);

// URL inputs here are application paths. Absolute external URLs, blobs and data
// stay intact. Router paths stay logical and BrowserRouter adds the basename.
export function appUrl(path: string, base = webBase): string {
  if (/^(?:[a-z][a-z\d+.-]*:|\/\/)/i.test(path)) return path;
  return `${base}${path.replace(/^\.?\//, '')}`;
}

export function appPathname(pathname: string, base = webBase): string {
  return base !== '/' && pathname.startsWith(base)
    ? `/${pathname.slice(base.length)}` : pathname;
}

// Supervisor wire paths are always rooted, even when the browser is mounted.
export function supervisorPath(path: string): string {
  return `/${path.replace(/^\//, '')}`;
}
