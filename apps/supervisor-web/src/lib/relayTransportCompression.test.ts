import { describe, expect, it } from 'vitest';
import { b64, gunzip, SocketCipher, type WireMessage } from './relayTransportCrypto';

const encoder = new TextEncoder();

async function gzip(text: string) {
  return new Response(
    new Response(encoder.encode(text)).body!.pipeThrough(new CompressionStream('gzip')),
  ).arrayBuffer();
}

function nonce(sequence: number) {
  const value = new Uint8Array(12);
  new DataView(value.buffer).setBigUint64(4, BigInt(sequence));
  return value;
}

// Mirrors Session::seal in crates/supervisor/src/secure_transport.rs.
async function sealServerEvent(key: CryptoKey, channel: string, sequence: number, message: WireMessage & { type: string }, compress: boolean): Promise<WireMessage> {
  const clear = JSON.stringify(message);
  const label = compress ? 'rcd-ws-gzip-v1' : 'rcd-ws-v1';
  const body = await crypto.subtle.encrypt(
    {
      name: 'AES-GCM',
      iv: nonce(sequence),
      additionalData: encoder.encode(`${label}\n${channel}\n${sequence}\n${message.type ?? ''}\n${message.threadId ?? ''}\n${message.shellId ?? ''}`),
    },
    key,
    compress ? await gzip(clear) : encoder.encode(clear),
  );
  return {
    type: message.type,
    threadId: message.threadId ?? null,
    shellId: message.shellId ?? null,
    encrypted: { version: 1, channelId: channel, sequence, body: b64(body), ...(compress ? { encoding: 'gzip' as const } : {}) },
  };
}

async function cipher() {
  const raw = crypto.getRandomValues(new Uint8Array(32));
  const server = await crypto.subtle.importKey('raw', raw, 'AES-GCM', false, ['encrypt']);
  const receive = await crypto.subtle.importKey('raw', raw, 'AES-GCM', false, ['decrypt']);
  const send = await crypto.subtle.importKey('raw', crypto.getRandomValues(new Uint8Array(32)), 'AES-GCM', false, ['encrypt']);
  return { server, client: new SocketCipher('channel', send, receive) };
}

describe('relay transport compression', () => {
  it('inflates gzip bodies and rejects bodies above the limit', async () => {
    const text = JSON.stringify({ items: Array(500).fill('tool output line') });
    const compressed = await gzip(text);
    expect(compressed.byteLength).toBeLessThan(text.length / 5);
    expect(new TextDecoder().decode(await gunzip(compressed, text.length))).toBe(text);
    await expect(gunzip(compressed, text.length - 1)).rejects.toThrow();
  });

  it('opens gzipped and plain server events and binds the encoding marker', async () => {
    const { server, client } = await cipher();
    const large = { type: 'thread.item.completed', threadId: 't', payload: { text: 'output '.repeat(1000) } };
    const sealed = await sealServerEvent(server, 'channel', 0, large, true);
    const { encoding: _encoding, ...unmarked } = sealed.encrypted!;
    const stripped = { ...sealed, encrypted: unmarked };
    await expect(client.open(stripped)).rejects.toThrow();
    expect(await client.open(sealed)).toEqual(large);

    const small = { type: 'supervisor.pong', threadId: null, shellId: null, payload: {} };
    const plain = await sealServerEvent(server, 'channel', 1, small, false);
    await expect(client.open({ ...plain, encrypted: { ...plain.encrypted!, encoding: 'gzip' } })).rejects.toThrow();
    await expect(client.open({ ...plain, encrypted: { ...plain.encrypted!, encoding: 'br' as 'gzip' } })).rejects.toThrow();
    expect(await client.open(plain)).toEqual(small);
  });
});
