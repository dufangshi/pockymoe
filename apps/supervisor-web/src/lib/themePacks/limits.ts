import { packError } from './errors';

/** Limits for one pack. Built-in packs obey them too. */
export const THEME_PACK_LIMITS = {
  /** Files in the pack, licences and the manifest included. */
  files: 64,
  /** Uncompressed size of all files. */
  totalBytes: 8 * 1024 * 1024,
  /** The archive itself. */
  archiveBytes: 10 * 1024 * 1024,
  stylesheetBytes: 1024 * 1024,
  manifestBytes: 64 * 1024,
  pathLength: 200,
  /** Nesting of CSS blocks. */
  cssDepth: 24,
} as const;

export const MANIFEST_FILE = 'theme.json';

const IMAGE_TYPES: Record<string, string> = {
  webp: 'image/webp',
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  avif: 'image/avif',
  svg: 'image/svg+xml',
};
const FONT_TYPES: Record<string, string> = { woff2: 'font/woff2', woff: 'font/woff' };
const TEXT_TYPES: Record<string, string> = { css: 'text/css', json: 'application/json', txt: 'text/plain', md: 'text/markdown' };

export type PackFileKind = 'manifest' | 'stylesheet' | 'image' | 'font' | 'text';

function extension(path: string) {
  const name = path.split('/').pop()!;
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
}

/** Licence files may come without an extension (`LICENSE`, `OFL`). */
const LICENCE_NAME = /^(?:licen[cs]e|copying|notice|ofl)(?:[-_.][a-z0-9-]+)?$/i;

export function packFileKind(path: string): PackFileKind | null {
  if (path === MANIFEST_FILE) return 'manifest';
  const ext = extension(path);
  if (ext === 'css') return 'stylesheet';
  if (IMAGE_TYPES[ext]) return 'image';
  if (FONT_TYPES[ext]) return 'font';
  if (ext === 'txt' || ext === 'md' || LICENCE_NAME.test(path.split('/').pop()!)) return 'text';
  return null;
}

export function packFileType(path: string) {
  const ext = extension(path);
  return IMAGE_TYPES[ext] ?? FONT_TYPES[ext] ?? TEXT_TYPES[ext] ?? 'text/plain';
}

/** A relative path inside a pack: forward slashes, no `.`/`..` segments, no drive
 *  letters, query strings, quotes or control characters. */
export function normalizePackPath(raw: string, context = 'path'): string {
  if (typeof raw !== 'string' || !raw) packError('path', `${context}: empty path`);
  if (raw.length > THEME_PACK_LIMITS.pathLength) packError('path', `${context}: path is too long`);
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f\\"'`?#%<>|*:]/.test(raw)) packError('path', `${raw}: unsupported character`);
  if (raw.startsWith('/') || /^[a-z]:/i.test(raw)) packError('path', `${raw}: absolute path`);
  const segments = raw.split('/');
  if (segments.some((segment) => segment === '..')) packError('path', `${raw}: parent directory`);
  if (segments.some((segment) => segment === '' || segment === '.' || segment.endsWith(' ') || segment.endsWith('.')))
    packError('path', `${raw}: empty or dotted segment`);
  return raw;
}

export function isImagePath(path: string) {
  return packFileKind(path) === 'image';
}
export function isFontPath(path: string) {
  return packFileKind(path) === 'font';
}

/** Checks that a binary asset starts like its extension says. */
export function checkAssetSignature(path: string, bytes: Uint8Array) {
  const ext = extension(path);
  const starts = (...values: number[]) => values.every((value, index) => bytes[index] === value);
  const ascii = (offset: number, text: string) => [...text].every((char, index) => bytes[offset + index] === char.charCodeAt(0));
  const ok =
    ext === 'png' ? starts(0x89, 0x50, 0x4e, 0x47)
    : ext === 'jpg' || ext === 'jpeg' ? starts(0xff, 0xd8, 0xff)
    : ext === 'gif' ? ascii(0, 'GIF8')
    : ext === 'webp' ? ascii(0, 'RIFF') && ascii(8, 'WEBP')
    : ext === 'woff2' ? ascii(0, 'wOF2')
    : ext === 'woff' ? ascii(0, 'wOFF')
    : ext === 'avif' ? ascii(4, 'ftyp')
    : ext === 'svg' ? /<svg[\s>]/i.test(new TextDecoder().decode(bytes.subarray(0, 4096)))
    : true;
  if (!ok) packError('asset', `${path}: content does not match its file type`);
}
