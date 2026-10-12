import type { Plugin } from 'vite';

// Header/rewrite files consumed by static hosts (for example Cloudflare Pages).
export function staticHosting(relay: string): Plugin {
  let origin = '';
  let websocket = '';
  if (relay.trim()) {
    const url = new URL(relay);
    if (!(url.protocol === 'https:' || (url.protocol === 'http:' && ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname))) ||
        url.pathname !== '/' || url.username || url.password || url.search || url.hash)
      throw new Error('VITE_POCKYMOE_RELAY_ORIGIN must be a secure origin');
    origin = url.origin;
    websocket = `${url.protocol === 'https:' ? 'wss:' : 'ws:'}//${url.host}`;
  }
  const csp = `default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob: ${origin}; media-src 'self' blob: ${origin}; font-src 'self' data:; connect-src 'self' ${origin} ${websocket}; worker-src 'self' blob:; frame-src 'self' blob:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self' ${origin}`;
  return {
    name: 'static-host-security',
    apply: 'build',
    generateBundle() {
      this.emitFile({ type: 'asset', fileName: '_headers', source: `/*\n  Content-Security-Policy: ${csp}\n  X-Content-Type-Options: nosniff\n  Referrer-Policy: no-referrer\n  X-Frame-Options: DENY\n/assets/*\n  Service-Worker-Allowed: /\n/index.html\n  Cache-Control: no-cache\n` });
      this.emitFile({ type: 'asset', fileName: '_redirects', source: '/* /index.html 200\n' });
      this.emitFile({ type: 'asset', fileName: 'STATIC_HOSTING.txt', source: `Serve this directory over HTTPS on the trusted Web origin.\nApply _headers as HTTP response headers, including the CSP; do not serve these rules as ordinary public files.\nApply _redirects as a fallback rewrite to index.html only when no static file exists.\nIf the host does not support these files, translate them into its response-header and SPA-rewrite configuration.\nService-Worker-Allowed: / is required on the built relayServiceWorker asset.\nNo inline script is needed: this build selects relay mode at compile time.\nRelay API origin: ${origin || '(same origin)'}\nSet POCKYMOE_WEB_ORIGIN on the relay to the trusted Web origin. Both origins must share the same registrable domain (same site) because relay cookies remain host-only, Secure, SameSite=Lax. Localhost with different ports is suitable for local development.\nKeep the old device /supervisor/tunnel route reachable when moving an existing installation. Deployment, DNS, CDN rules and workflows are intentionally separate.\n` });
    },
  };
}
