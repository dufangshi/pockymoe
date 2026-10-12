import { describe, expect, it } from 'vitest';

import { ThemePackError } from './errors';
import { THEME_PACK_LIMITS, normalizePackPath } from './limits';
import { localizedText, validateManifest } from './manifest';
import { compileThemePack, readThemePackArchive, validatePackFiles, writeThemePackArchive } from './pack';
import { PNG, WEBP, testManifest, testPackFiles, text } from './testPack';
import { readZip, writeZip } from './zip';

function rejection(run: () => unknown) {
  try {
    run();
  } catch (error) {
    if (error instanceof ThemePackError) return { code: error.code, detail: error.detail };
    throw error;
  }
  throw new Error('expected a ThemePackError');
}
async function asyncRejection(run: () => Promise<unknown>) {
  try {
    await run();
  } catch (error) {
    if (error instanceof ThemePackError) return { code: error.code, detail: error.detail };
    throw error;
  }
  throw new Error('expected a ThemePackError');
}

describe('theme.json', () => {
  it('accepts a full manifest, fills defaults and keeps unknown flags', () => {
    const manifest = validateManifest({ ...testManifest({ modes: undefined, features: ['composer.richMenus', 'composer.richMenus', 'future.flag'] }), unknownKey: true });
    expect(manifest).toMatchObject({ id: 'test-pack', modes: ['light', 'dark'], features: ['composer.richMenus', 'future.flag'], chrome: { light: '#112233', dark: '#000000' } });
    expect(manifest).not.toHaveProperty('unknownKey');
  });

  it.each([
    ['schema', { schema: 2 }, 'manifest', /schema 2/],
    ['id format', { id: 'Plum Pocket' }, 'manifest', /id must be/],
    ['reserved id', { id: 'classic' }, 'reserved', /reserved/],
    ['version', { version: 'v1' }, 'manifest', /version/],
    ['name', { name: '' }, 'manifest', /name/],
    ['locale', { name: { english: 'x' } }, 'manifest', /not a locale/],
    ['swatch', { swatches: ['red'] }, 'manifest', /swatches\[0\] must be a hex color/],
    ['chrome mode', { chrome: { sepia: '#000000' } }, 'manifest', /chrome\.sepia/],
    ['no styles', { styles: [] }, 'manifest', /at least one stylesheet/],
    ['style type', { styles: ['styles/main.js'] }, 'manifest', /must be a \.css file/],
    ['duplicate style', { styles: ['styles/main.css', 'styles/main.css'] }, 'manifest', /twice/],
    ['font traversal', { fonts: [{ family: 'X', src: '../x.woff2' }] }, 'manifest', /not a safe relative path/],
    ['font family', { fonts: [{ family: 'X"; }', src: 'assets/test.woff2' }] }, 'manifest', /family/],
    ['font weight', { fonts: [{ family: 'X', src: 'assets/test.woff2', weight: 'bold' }] }, 'manifest', /weight/],
    ['agent key', { agents: { Claude: { avatar: 'assets/claude.webp' } } }, 'manifest', /not an agent key/],
    ['agent art type', { agents: { claude: { avatar: 'styles/main.css' } } }, 'manifest', /must be an image/],
    ['feature name', { features: ['not a flag'] }, 'manifest', /features\[0\]/],
  ])('rejects a bad %s', (_name, overrides, code, detail) => {
    const error = rejection(() => validateManifest(testManifest(overrides)));
    expect(error.code).toBe(code);
    expect(error.detail).toMatch(detail);
  });

  it('picks localized text by locale, language, English, then any', () => {
    const value = { en: 'Plum', 'zh-CN': '梅子', fr: 'Prune' };
    expect(localizedText(value, 'zh-CN')).toBe('梅子');
    expect(localizedText(value, 'zh-TW')).toBe('梅子');
    expect(localizedText(value, 'de')).toBe('Plum');
    expect(localizedText({ fr: 'Prune' }, 'de')).toBe('Prune');
    expect(localizedText('Plain', 'zh-CN')).toBe('Plain');
  });
});

describe('pack files', () => {
  it('validates a pack and compiles generated font and agent rules', () => {
    const pack = validatePackFiles(testPackFiles());
    const css = compileThemePack(pack, (path) => `/packs/${path}`);
    expect(css).toContain('@font-face { font-family: "test-pack--Test Sans"; src: url("/packs/assets/test.woff2") format("woff2"); font-weight: 400 700;');
    expect(css).toContain(':root[data-theme-preset="test-pack"], :root[data-theme-preset="test-pack"] [data-agent] { --theme-agent-avatar: url("/packs/assets/agent.webp"); --theme-agent-peek: none; }');
    expect(css).toContain(':root[data-theme-preset="test-pack"] [data-agent="claude"] { --theme-agent-avatar: url("/packs/assets/claude.webp"); --theme-agent-peek: url("/packs/assets/claude.webp"); }');
  });

  it.each([
    ['parent directory', '../evil.css', 'path'],
    ['nested parent directory', 'styles/../../evil.css', 'path'],
    ['absolute path', '/etc/evil.css', 'path'],
    ['drive letter', 'C:/evil.css', 'path'],
    ['backslash', 'styles\\evil.css', 'path'],
    ['dot segment', 'styles/./evil.css', 'path'],
    ['script', 'assets/evil.js', 'asset'],
    ['html', 'assets/evil.html', 'asset'],
  ])('rejects a file with a %s', (_name, path, code) => {
    expect(rejection(() => validatePackFiles(testPackFiles({ extra: [[path, text('x')]] }))).code).toBe(code);
  });

  it('rejects duplicate names that differ only in case', () => {
    expect(rejection(() => validatePackFiles(testPackFiles({ extra: [['assets/Agent.webp', WEBP]] }))).detail).toMatch(/duplicate/);
  });

  it('enforces the file, size and stylesheet limits', () => {
    const many = Array.from({ length: THEME_PACK_LIMITS.files }, (_, index): [string, Uint8Array] => [`assets/a${index}.png`, PNG]);
    expect(rejection(() => validatePackFiles(testPackFiles({ extra: many }))).code).toBe('limits');
    const big = new Uint8Array(THEME_PACK_LIMITS.totalBytes);
    big.set(PNG);
    expect(rejection(() => validatePackFiles(testPackFiles({ extra: [['assets/big.png', big]] }))).code).toBe('limits');
    const sheet = text(`:theme { color: red; }${' '.repeat(THEME_PACK_LIMITS.stylesheetBytes)}`);
    expect(rejection(() => validatePackFiles(testPackFiles({ extra: [['styles/big.css', sheet]] }))).detail).toMatch(/larger than 1 MB/);
  });

  it('checks that images and fonts are what their extension says', () => {
    expect(rejection(() => validatePackFiles(testPackFiles({ extra: [['assets/fake.png', text('<script>')]] }))).code).toBe('asset');
    expect(rejection(() => validatePackFiles(testPackFiles({ extra: [['assets/fake.svg', text('<html></html>')]] }))).code).toBe('asset');
    expect(() => validatePackFiles(testPackFiles({ extra: [['assets/ok.svg', text('<svg xmlns="http://www.w3.org/2000/svg"></svg>')]] }))).not.toThrow();
  });

  it('requires theme.json and every file it names', () => {
    expect(rejection(() => validatePackFiles(testPackFiles().filter(([name]) => name !== 'theme.json'))).code).toBe('archive');
    expect(rejection(() => validatePackFiles(testPackFiles().filter(([name]) => name !== 'assets/claude.webp'))).detail).toMatch(/assets\/claude\.webp is listed/);
    expect(rejection(() => validatePackFiles(testPackFiles({ extra: [] }).map(([name, bytes]) => [name, name === 'theme.json' ? text('{') : bytes])))).toMatchObject({ code: 'manifest' });
  });

  it('normalizes nothing silently: unsafe paths throw', () => {
    expect(normalizePackPath('assets/a.png')).toBe('assets/a.png');
    for (const path of ['', 'a//b.png', 'a/b.png?x', 'a/b.png#x', 'a/%2e%2e/b.png', 'a/b.png ']) expect(() => normalizePackPath(path)).toThrow(ThemePackError);
  });
});

describe('.pockytheme archives', () => {
  it('round-trips a pack with the manifest first and deterministic bytes', async () => {
    const pack = validatePackFiles(testPackFiles());
    const bytes = await writeThemePackArchive(pack);
    expect([...(await writeThemePackArchive(pack))]).toEqual([...bytes]);
    const entries = await readZip(bytes);
    expect(entries[0]!.name).toBe('theme.json');
    const read = await readThemePackArchive(bytes);
    expect(read.manifest).toEqual(pack.manifest);
    expect([...read.files.keys()].sort()).toEqual([...pack.files.keys()].sort());
    for (const [name, file] of pack.files) expect([...read.files.get(name)!]).toEqual([...file]);
  });

  it('accepts a zipped pack folder and ignores archiver litter', async () => {
    const bytes = await writeZip([
      ...testPackFiles().map(([name, data]) => ({ name: `my-pack/${name}`, bytes: data })),
      { name: '__MACOSX/my-pack/._theme.json', bytes: text('x') },
      { name: 'my-pack/.DS_Store', bytes: text('x') },
    ]);
    expect((await readThemePackArchive(bytes)).manifest.id).toBe('test-pack');
  });

  it('rejects traversal names inside the archive', async () => {
    const bytes = await writeZip([...testPackFiles().map(([name, data]) => ({ name, bytes: data })), { name: '../../evil.css', bytes: text(':theme{}') }]);
    expect(await asyncRejection(() => readThemePackArchive(bytes))).toMatchObject({ code: 'path' });
  });

  it('rejects archives that are not zips, too large, encrypted or lying about sizes', async () => {
    expect((await asyncRejection(() => readZip(text('not a zip')))).code).toBe('archive');
    expect((await asyncRejection(() => readZip(new Uint8Array(THEME_PACK_LIMITS.archiveBytes + 1)))).code).toBe('limits');

    const css = text(`:theme { color: red; }\n${'a'.repeat(50_000)}`);
    const zip = await writeZip([{ name: 'styles/main.css', bytes: css }]);
    const view = new DataView(zip.buffer);
    const central = zip.length - 22 - (46 + 'styles/main.css'.length);
    expect(view.getUint32(central, true)).toBe(0x02014b50);
    expect(view.getUint16(central + 10, true)).toBe(8);

    // A deflated entry that inflates beyond its declared size stops early.
    const bomb = zip.slice();
    new DataView(bomb.buffer).setUint32(central + 24, 100, true);
    expect(await asyncRejection(() => readZip(bomb))).toMatchObject({ code: 'limits', detail: expect.stringMatching(/inflates beyond/) });

    // Declared sizes are summed before anything is inflated.
    const huge = zip.slice();
    new DataView(huge.buffer).setUint32(central + 24, THEME_PACK_LIMITS.totalBytes + 1, true);
    expect((await asyncRejection(() => readZip(huge))).code).toBe('limits');

    const encrypted = zip.slice();
    new DataView(encrypted.buffer).setUint16(central + 8, 0x0801, true);
    expect(await asyncRejection(() => readZip(encrypted))).toMatchObject({ code: 'archive', detail: expect.stringMatching(/encrypted/) });

    const corrupt = zip.slice();
    new DataView(corrupt.buffer).setUint32(central + 16, 0, true);
    expect(await asyncRejection(() => readZip(corrupt))).toMatchObject({ code: 'archive', detail: expect.stringMatching(/checksum/) });
  });

  it('caps the number of entries', async () => {
    const entries = Array.from({ length: THEME_PACK_LIMITS.files + 1 }, (_, index) => ({ name: `assets/a${index}.png`, bytes: PNG }));
    expect((await asyncRejection(async () => readZip(await writeZip(entries)))).code).toBe('limits');
  });

  it('keeps binary assets stored as they are', async () => {
    const zip = await writeZip([{ name: 'assets/a.webp', bytes: WEBP }]);
    expect([...(await readZip(zip))[0]!.bytes]).toEqual([...WEBP]);
  });
});
