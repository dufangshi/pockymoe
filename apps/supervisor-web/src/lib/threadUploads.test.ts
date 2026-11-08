import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError, deviceRequest } from './api';
import { prepareAttachments } from './threadUploads';

vi.mock('./api', async (original) => ({ ...(await original<typeof import('./api')>()), deviceRequest: vi.fn() }));
const request = vi.mocked(deviceRequest);
const MB = 1024 * 1024;

function attachment(size: number, clientId = 'a', placeholder = '[FILE 1]') {
  return { clientId, kind: 'file' as const, originalName: 'big.bin', placeholder, file: new File([new Uint8Array(size)], 'big.bin') };
}
const never = new AbortController().signal;

/** A device that takes chunks; `fail` may answer one request instead. */
function device(fail?: (path: string, method: string) => unknown) {
  let received = 0;
  const offsets: number[] = [];
  request.mockImplementation(async (_device, input, init) => {
    const path = String(input), method = init?.method ?? 'GET';
    const failure = fail?.(path, method);
    if (failure) throw failure;
    if (method === 'POST' && path.endsWith('/uploads')) return { uploadId: 'u1', chunkSize: 4 * MB } as never;
    if (method === 'PUT') {
      offsets.push(Number(new URL(path, 'http://x').searchParams.get('offset')));
      received += (init!.body as Blob).size;
      return { received } as never;
    }
    if (path.endsWith('/complete')) return { token: '[FILE ./.temp/threads/t/big.bin]' } as never;
    return { received } as never;
  });
  return { offsets, set: (value: number) => { received = value; } };
}

describe('prepareAttachments', () => {
  beforeEach(() => { request.mockReset(); });

  it('leaves what fits one request to the prompt itself', async () => {
    const input = { prompt: 'x [FILE 1]', attachments: [attachment(MB)] };
    expect(await prepareAttachments('t', input, null, never, () => undefined)).toBe(input);
    expect(request).not.toHaveBeenCalled();
  });

  it('sends a large file in chunks and names it in the prompt', async () => {
    const { offsets } = device();
    const result = await prepareAttachments('t', { prompt: 'x [FILE 1]', attachments: [attachment(17 * MB)] }, null, never, () => undefined);
    expect(offsets).toEqual([0, 4 * MB, 8 * MB, 12 * MB, 16 * MB]);
    expect(result.prompt).toBe('x [FILE ./.temp/threads/t/big.bin]');
    expect(result.attachments).toBeUndefined();
  });

  it('refuses a file over 1 GB without sending anything', async () => {
    const huge = attachment(1);
    Object.defineProperty(huge.file, 'size', { value: 1024 * MB + 1 });
    await expect(prepareAttachments('t', { prompt: '[FILE 1]', attachments: [huge] }, null, never, () => undefined)).rejects.toThrow();
    expect(request).not.toHaveBeenCalled();
  });

  it('continues from where the device is when a chunk conflicts', async () => {
    let conflicted = false;
    const state = device((_path, method) => {
      if (method === 'PUT' && !conflicted) {
        conflicted = true;
        state.set(4 * MB); // The first try landed; only its answer was lost.
        return new ApiError(409, { code: 'offset_conflict', message: 'conflict' } as never);
      }
      return null;
    });
    await prepareAttachments('t', { prompt: '[FILE 1]', attachments: [attachment(17 * MB, 'conflict')] }, null, never, () => undefined);
    expect(state.offsets).toEqual([4 * MB, 8 * MB, 12 * MB, 16 * MB]);
  });

  it('falls back to one request on a device without the routes, and says when it cannot', async () => {
    request.mockImplementation(() => Promise.reject(new ApiError(404, { code: 'not_found', message: 'nope' } as never)));
    const fits = { prompt: '[FILE 1]', attachments: [attachment(20 * MB, 'old')] };
    expect(await prepareAttachments('t', fits, null, never, () => undefined)).toBe(fits);
    const tooLarge = await prepareAttachments('t', { prompt: '[FILE 1]', attachments: [attachment(26 * MB, 'old2')] }, null, never, () => undefined).catch((error: Error) => error);
    expect(tooLarge).toBeInstanceOf(Error);
    expect((tooLarge as Error).message).not.toBe('nope');
  });
});
