import { describe, expect, it } from 'vitest';
import { staticHosting } from './staticHosting';

describe('static host WebAssembly policy', () => {
  it('permits Argon2id WebAssembly without allowing JavaScript eval or inline scripts', () => {
    const plugin = staticHosting('https://relay.example.test');
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
});
