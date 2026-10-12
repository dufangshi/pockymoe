/* Minimal zip reader and writer for theme packs. Stored and deflated entries only
   (no zip64, encryption or multi-disk archives); deflate goes through the
   browser's DecompressionStream('deflate-raw'). Sizes are checked against the
   pack limits before and while inflating, so a zip bomb stops early. */

import { packError } from './errors';
import { THEME_PACK_LIMITS } from './limits';

export interface ZipEntry {
  name: string;
  bytes: Uint8Array;
}

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n += 1) {
    let c = n;
    for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  return table;
})();

export function crc32(bytes: Uint8Array) {
  let crc = 0xffffffff;
  for (let index = 0; index < bytes.length; index += 1) crc = CRC_TABLE[(crc ^ bytes[index]!) & 0xff]! ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

async function transform(bytes: Uint8Array, stream: CompressionStream | DecompressionStream, limit: number) {
  const source = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(bytes);
      controller.close();
    },
  });
  const reader = source.pipeThrough(stream as unknown as TransformStream<Uint8Array, Uint8Array>).getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.length;
    if (total > limit) {
      await reader.cancel().catch(() => undefined);
      packError('limits', 'a file inflates beyond its declared size');
    }
    chunks.push(value);
  }
  const out = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return out;
}

/** Names that archivers add and packs never need. */
function ignoredEntry(name: string) {
  return name.endsWith('/') || name.startsWith('__MACOSX/') || name.split('/').pop() === '.DS_Store' || name.split('/').pop() === 'Thumbs.db';
}

/** Reads every file entry. Paths are returned as stored; the caller validates them. */
export async function readZip(input: Uint8Array): Promise<ZipEntry[]> {
  if (input.length > THEME_PACK_LIMITS.archiveBytes) packError('limits', 'the archive is larger than 10 MB');
  const view = new DataView(input.buffer, input.byteOffset, input.byteLength);
  let eocd = -1;
  for (let at = input.length - 22; at >= Math.max(0, input.length - 22 - 0xffff); at -= 1) {
    if (view.getUint32(at, true) === 0x06054b50) {
      eocd = at;
      break;
    }
  }
  if (eocd < 0) packError('archive', 'no zip directory found');
  const disk = view.getUint16(eocd + 4, true);
  const count = view.getUint16(eocd + 10, true);
  const directorySize = view.getUint32(eocd + 12, true);
  const directoryOffset = view.getUint32(eocd + 16, true);
  if (disk !== 0 || count === 0xffff || directoryOffset === 0xffffffff) packError('archive', 'multi-disk and zip64 archives are not supported');
  if (directoryOffset + directorySize > eocd) packError('archive', 'the zip directory is out of range');
  if (count > THEME_PACK_LIMITS.files * 4) packError('limits', `more than ${THEME_PACK_LIMITS.files} files`);

  const decoder = new TextDecoder('utf-8', { fatal: true });
  const records: { name: string; method: number; crc: number; compressed: number; size: number; local: number }[] = [];
  let at = directoryOffset;
  let declaredTotal = 0;
  for (let index = 0; index < count; index += 1) {
    if (at + 46 > eocd || view.getUint32(at, true) !== 0x02014b50) packError('archive', 'corrupt zip directory');
    const flags = view.getUint16(at + 8, true);
    const method = view.getUint16(at + 10, true);
    const crc = view.getUint32(at + 16, true);
    const compressed = view.getUint32(at + 20, true);
    const size = view.getUint32(at + 24, true);
    const nameLength = view.getUint16(at + 28, true);
    const extraLength = view.getUint16(at + 30, true);
    const commentLength = view.getUint16(at + 32, true);
    const local = view.getUint32(at + 42, true);
    let name: string;
    try {
      name = decoder.decode(input.subarray(at + 46, at + 46 + nameLength));
    } catch {
      packError('archive', 'a file name is not UTF-8');
    }
    at += 46 + nameLength + extraLength + commentLength;
    if (ignoredEntry(name)) continue;
    if (flags & 0x1) packError('archive', `${name}: encrypted entries are not supported`);
    if (method !== 0 && method !== 8) packError('archive', `${name}: compression method ${method} is not supported`);
    if (size === 0xffffffff || compressed === 0xffffffff) packError('archive', 'zip64 entries are not supported');
    declaredTotal += size;
    if (declaredTotal > THEME_PACK_LIMITS.totalBytes) packError('limits', 'the files add up to more than 8 MB');
    records.push({ name, method, crc, compressed, size, local });
  }
  if (records.length > THEME_PACK_LIMITS.files) packError('limits', `more than ${THEME_PACK_LIMITS.files} files`);

  const entries: ZipEntry[] = [];
  for (const record of records) {
    if (record.local + 30 > input.length || view.getUint32(record.local, true) !== 0x04034b50) packError('archive', `${record.name}: corrupt local header`);
    const start = record.local + 30 + view.getUint16(record.local + 26, true) + view.getUint16(record.local + 28, true);
    if (start + record.compressed > input.length) packError('archive', `${record.name}: data is out of range`);
    const data = input.subarray(start, start + record.compressed);
    let bytes: Uint8Array;
    if (record.method === 0) {
      if (record.compressed !== record.size) packError('archive', `${record.name}: stored size mismatch`);
      bytes = data.slice();
    } else {
      try {
        bytes = await transform(data, new DecompressionStream('deflate-raw'), record.size);
      } catch (error) {
        if (error instanceof Error && error.name === 'ThemePackError') throw error;
        packError('archive', `${record.name}: corrupt deflate data`);
      }
    }
    if (bytes.length !== record.size || crc32(bytes) !== record.crc) packError('archive', `${record.name}: checksum mismatch`);
    entries.push({ name: record.name, bytes });
  }
  return entries;
}

const COMPRESSIBLE = /\.(?:css|json|txt|md|svg)$|(?:^|\/)(?:LICEN[CS]E|COPYING|OFL)[^/]*$/i;

/** Writes a zip; text files are deflated when the browser supports it. Entries get
 *  a fixed timestamp so the same pack always exports to the same bytes. */
export async function writeZip(entries: ZipEntry[]): Promise<Uint8Array> {
  const encoder = new TextEncoder();
  const locals: Uint8Array[] = [];
  const directory: Uint8Array[] = [];
  let offset = 0;
  for (const entry of entries) {
    const name = encoder.encode(entry.name);
    const crc = crc32(entry.bytes);
    let method = 0;
    let data = entry.bytes;
    if (COMPRESSIBLE.test(entry.name) && typeof CompressionStream === 'function') {
      const deflated = await transform(entry.bytes, new CompressionStream('deflate-raw'), entry.bytes.length + 1024);
      if (deflated.length < entry.bytes.length) {
        method = 8;
        data = deflated;
      }
    }
    const header = new DataView(new ArrayBuffer(30));
    header.setUint32(0, 0x04034b50, true);
    header.setUint16(4, 20, true);
    header.setUint16(6, 0x0800, true); // UTF-8 names
    header.setUint16(8, method, true);
    header.setUint16(10, 0, true); // 00:00
    header.setUint16(12, 0x21, true); // 1980-01-01
    header.setUint32(14, crc, true);
    header.setUint32(18, data.length, true);
    header.setUint32(22, entry.bytes.length, true);
    header.setUint16(26, name.length, true);
    locals.push(new Uint8Array(header.buffer), name, data);

    const central = new DataView(new ArrayBuffer(46));
    central.setUint32(0, 0x02014b50, true);
    central.setUint16(4, 20, true);
    central.setUint16(6, 20, true);
    central.setUint16(8, 0x0800, true);
    central.setUint16(10, method, true);
    central.setUint16(12, 0, true);
    central.setUint16(14, 0x21, true);
    central.setUint32(16, crc, true);
    central.setUint32(20, data.length, true);
    central.setUint32(24, entry.bytes.length, true);
    central.setUint16(28, name.length, true);
    central.setUint32(42, offset, true);
    directory.push(new Uint8Array(central.buffer), name);
    offset += 30 + name.length + data.length;
  }
  const directorySize = directory.reduce((sum, part) => sum + part.length, 0);
  const end = new DataView(new ArrayBuffer(22));
  end.setUint32(0, 0x06054b50, true);
  end.setUint16(8, entries.length, true);
  end.setUint16(10, entries.length, true);
  end.setUint32(12, directorySize, true);
  end.setUint32(16, offset, true);
  const parts = [...locals, ...directory, new Uint8Array(end.buffer)];
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}
