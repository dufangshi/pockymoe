import { describe, expect, it } from 'vitest';
import { staticHosting } from './staticHosting';

describe('static host WebAssembly policy', () => {
  it('permits Argon2id WebAssembly without allowing JavaScript eval or inline scripts', () => {
    const plugin = staticHosting('https://relay.example.test', 'web.example.test');
    const assets: Array<{ fileName: string; source: string }> = [];
    const hook = plugin.generateBundle;
    const handler = typeof hook === 'function' ? hook : hook?.handler;
    (handler as Function).call({ emitFile: (asset: { fileName: string; source: string }) => assets.push(asset) });
    const headers = assets.find(asset => asset.fileName === '_headers')!.source;
    expect(headers).toContain("script-src 'self' 'wasm-unsafe-eval';");
    expect(headers).not.toContain("'unsafe-eval'");
    expect(headers).not.toMatch(/script-src[^;]*'unsafe-inline'/);
    expect(headers).toContain("connect-src 'self' https://relay.example.test wss://relay.example.test;");
  });
  it('puts the same CSP in the HTML except for frame-ancestors and writes the domain', () => {
    const plugin = staticHosting('https://relay.example.test', 'WEB.example.test');
    const transform = plugin.transformIndexHtml;
    if (!transform || typeof transform === 'function' || !('handler' in transform)) throw new Error('Missing HTML transform');
    const tags = (transform.handler as Function)();
    expect(tags[0]).toMatchObject({ tag: 'meta', injectTo: 'head-prepend', attrs: {
      'http-equiv': 'Content-Security-Policy',
    } });
    expect(tags[0].attrs.content).not.toContain('frame-ancestors');
    expect(tags[0].attrs.content).toContain("script-src 'self' 'wasm-unsafe-eval';");
    expect(tags[1]).toEqual({ tag: 'meta', attrs: { name: 'referrer', content: 'no-referrer' }, injectTo: 'head-prepend' });
    const assets: Array<{ fileName: string; source: string }> = [];
    (plugin.generateBundle as Function).call({ emitFile: (asset: { fileName: string; source: string }) => assets.push(asset) });
    const headerCsp = assets.find(asset => asset.fileName === '_headers')!.source.match(/Content-Security-Policy: ([^\n]+)/)![1];
    expect(tags[0].attrs.content).toBe(headerCsp.replace("; frame-ancestors 'none'", ''));
    expect(assets.find(asset => asset.fileName === 'CNAME')!.source).toBe('web.example.test\n');
    expect(assets.find(asset => asset.fileName === '_headers')!.source).not.toContain('Service-Worker-Allowed');
  });
  it.each(['', 'https://web.example.test', 'web.example.test/path', 'web.example.test\nother.test', '../web', '-bad.example.test'])('rejects invalid CNAME configuration (%s)', domain => {
    expect(() => staticHosting('https://relay.example.test', domain)).toThrow('POCKYMOE_WEB_DOMAIN');
  });
});
