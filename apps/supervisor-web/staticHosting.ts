import type { Plugin } from 'vite';
import path from 'node:path';
import { copyFileSync } from 'node:fs';

// Header/rewrite files consumed by static hosts (for example Cloudflare Pages).
export function staticHosting(relay: string, domain: string): Plugin {
  if (domain.length > 253 || !domain.includes('.') ||
      !domain.split('.').every(label => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/i.test(label)))
    throw new Error('POCKYMOE_WEB_DOMAIN must be a DNS hostname');
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
    transformIndexHtml: {
      order: 'post',
      handler() {
        return [{ tag: 'meta', attrs: {
          'http-equiv': 'Content-Security-Policy',
          content: csp.replace("; frame-ancestors 'none'", ''),
        }, injectTo: 'head-prepend' }, {
          tag: 'meta', attrs: { name: 'referrer', content: 'no-referrer' }, injectTo: 'head-prepend',
        }];
      },
    },
    generateBundle() {
      this.emitFile({ type: 'asset', fileName: 'CNAME', source: `${domain.toLowerCase()}\n` });
      this.emitFile({ type: 'asset', fileName: '_headers', source: `/*\n  Content-Security-Policy: ${csp}\n  X-Content-Type-Options: nosniff\n  Referrer-Policy: no-referrer\n  X-Frame-Options: DENY\n/relayServiceWorker.js\n  Cache-Control: no-cache\n/index.html\n  Cache-Control: no-cache\n` });
      this.emitFile({ type: 'asset', fileName: '_redirects', source: '/* /index.html 200\n' });
      this.emitFile({ type: 'asset', fileName: 'STATIC_HOSTING.txt', source: `Serve this directory over HTTPS on the trusted Web origin.\nGitHub Pages uses CNAME and 404.html (a copy of index.html) for the domain and SPA fallback.\nThe HTML meta CSP applies on hosts without response headers; the app refuses to initialize in a frame. No inline script is needed.\nOn other hosts apply _headers as response headers and _redirects as a fallback rewrite only when no static file exists.\nThe service worker is /relayServiceWorker.js with scope /; no Service-Worker-Allowed header is needed.\nBuild CNAME: ${domain.toLowerCase()}\nRelay API origin: ${origin || '(same origin)'}\nSet POCKYMOE_WEB_ORIGIN on the relay to the trusted Web origin. Both origins must share the same registrable domain (same site) because relay cookies remain host-only, Secure, SameSite=Lax. Localhost with different ports is suitable for local development.\nKeep the old device /supervisor/tunnel route reachable when moving an existing installation.\n` });
    },
    writeBundle(options) {
      const output = options.dir ?? path.resolve('dist');
      copyFileSync(path.join(output, 'index.html'), path.join(output, '404.html'));
    },
  };
}
