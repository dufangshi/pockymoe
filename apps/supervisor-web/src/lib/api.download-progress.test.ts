import { afterEach, expect, it, vi } from 'vitest';
import { downloadWorkspaceFile } from './api';
import { nativeSave, NATIVE_SAVE_LIMIT } from '../desktop/download';

afterEach(() => {
  vi.unstubAllGlobals();
  delete (window as { pockymoeHost?: unknown }).pockymoeHost;
});

function streamed(parts: Uint8Array[], total: number | null) {
  const body = new ReadableStream<Uint8Array>({ start(controller) { for (const part of parts) controller.enqueue(part); controller.close(); } });
  return new Response(body, { headers: { 'content-type': 'application/zip', 'content-disposition': 'attachment; filename="big.zip"', ...(total === null ? {} : { 'content-length': String(total) }) } });
}

it('reports each received piece of a download and keeps every byte', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(streamed([new Uint8Array(3), new Uint8Array(5)], 8)));
  const progress: Array<[number, number | null]> = [];
  const result = await downloadWorkspaceFile('ws', { path: 'big.zip' }, 'local', undefined, undefined, (loaded, total) => progress.push([loaded, total]));
  expect(progress).toEqual([[0, 8], [3, 8], [8, 8]]);
  expect(result.blob.size).toBe(8);
  expect(result.blob.type).toBe('application/zip');
  expect(result.filename).toBe('big.zip');
});

it('reports progress without a total when the device sends none', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(streamed([new Uint8Array(4)], null)));
  const progress: Array<[number, number | null]> = [];
  await downloadWorkspaceFile('ws', { path: 'big.zip' }, 'local', undefined, undefined, (loaded, total) => progress.push([loaded, total]));
  expect(progress.at(-1)).toEqual([4, null]);
});

it('leaves a file over the desktop save limit to the download manager', async () => {
  const saveAs = vi.fn();
  (window as { pockymoeHost?: unknown }).pockymoeHost = { files: { saveAs }, capabilities: async () => ({ surface: 'desktop', features: { nativeSave: true } }) };
  const big = { size: NATIVE_SAVE_LIMIT + 1, arrayBuffer: vi.fn() } as unknown as Blob;
  expect(await nativeSave(big, 'big.zip')).toBe(false);
  expect(saveAs).not.toHaveBeenCalled();
  const small = { size: 5, arrayBuffer: async () => new ArrayBuffer(5) } as unknown as Blob;
  expect(await nativeSave(small, 'small.txt')).toBe(true);
  expect(saveAs).toHaveBeenCalledTimes(1);
});
