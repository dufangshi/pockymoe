/** Why a theme pack was rejected. The Settings dialog localizes the code and shows
 *  the detail (file, line and rule) after it. */
export type ThemePackErrorCode =
  | 'archive'
  | 'limits'
  | 'path'
  | 'manifest'
  | 'css'
  | 'asset'
  | 'reserved'
  | 'storage';

export class ThemePackError extends Error {
  constructor(
    readonly code: ThemePackErrorCode,
    readonly detail: string,
  ) {
    super(`${code}: ${detail}`);
    this.name = 'ThemePackError';
  }
}

export function packError(code: ThemePackErrorCode, detail: string): never {
  throw new ThemePackError(code, detail);
}
