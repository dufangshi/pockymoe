import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import path from 'node:path';

const root = process.argv[2] ?? 'apps/supervisor-web/dist';
const relay = new URL(process.env.VITE_POCKYMOE_RELAY_ORIGIN).origin;
const websocket = relay.replace(/^http/, 'ws');
const domain = process.env.POCKYMOE_WEB_DOMAIN;
assert.ok(domain, 'POCKYMOE_WEB_DOMAIN must be set');
const read = name => readFile(path.join(root, name), 'utf8');
const html = await read('index.html');
const referrer = html.match(/<meta\b[^>]*name="referrer"[^>]*content="no-referrer"[^>]*>/i)?.[0];
assert.ok(referrer, 'index.html must carry the no-referrer meta policy');
assert.ok(html.indexOf(referrer) < html.indexOf('<script'), 'referrer policy must precede executable scripts');
assert.ok(html.indexOf(referrer) < html.indexOf('<link'), 'referrer policy must precede resource links');
const meta = html.match(/<meta\b[^>]*http-equiv="Content-Security-Policy"[^>]*>/i)?.[0];
assert.ok(meta, 'index.html must carry the meta CSP');
const encodedCsp = meta.match(/content="([^"]+)"/)?.[1];
assert.ok(encodedCsp);
// Vite escapes HTML attribute values; compare the policy the browser reads.
const csp = encodedCsp.replace(/&#(\d+);/g, (_, code) => String.fromCodePoint(Number(code)))
  .replace(/&#x([\da-f]+);/gi, (_, code) => String.fromCodePoint(parseInt(code, 16)))
  .replace(/&quot;/g, '"').replace(/&apos;/g, "'").replace(/&amp;/g, '&');
assert.ok(html.indexOf(meta) < html.indexOf('<script'), 'CSP must precede executable scripts');
assert.ok(html.indexOf(meta) < html.indexOf('<link'), 'CSP must precede resource links');
const headerCsp = (await read('_headers')).match(/Content-Security-Policy: ([^\n]+)/)?.[1];
assert.ok(headerCsp);
assert.equal(csp, headerCsp.replace("; frame-ancestors 'none'", ''));
assert.ok(csp.includes("script-src 'self' 'wasm-unsafe-eval';"));
assert.ok(csp.includes(`connect-src 'self' ${relay} ${websocket};`));
assert.ok(!csp.includes('frame-ancestors'));
for (const script of html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/g)) {
  assert.match(script[1], /\bsrc="\//, 'all scripts must be external and same-origin');
  assert.equal(script[2].trim(), '', 'inline scripts are prohibited');
}
assert.equal(await read('404.html'), html, 'Pages fallback must match the entry point');
assert.equal(await read('CNAME'), `${domain.toLowerCase()}\n`);
const worker = await read('relayServiceWorker.js');
assert.ok(worker.length > 0, 'service worker must be emitted at the site root');
assert.ok(!(await read('_headers')).includes('Service-Worker-Allowed'));
const scripts = await Promise.all((await readdir(path.join(root, 'assets')))
  .filter(name => name.endsWith('.js')).map(name => read(`assets/${name}`)));
assert.ok(scripts.some(script => script.includes('/relayServiceWorker.js')), 'static bundle must register the root worker');
assert.ok(scripts.some(script => script.includes('The account Web app requires a top-level window')), 'static bundle must enforce frame protection');
console.log('PASS: meta CSP, no-referrer policy, external scripts, 404 fallback, CNAME, root service worker and frame protection');
