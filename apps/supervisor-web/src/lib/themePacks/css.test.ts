import { describe, expect, it } from 'vitest';

import { compileStylesheets } from './css';
import { ThemePackError } from './errors';

const ROOT = ':root[data-theme-preset="demo"]';

function compile(source: string, assets = ['assets/a.webp', 'assets/f.woff2'], fontFamilies: string[] = []) {
  return compileStylesheets([{ file: 'styles/main.css', source }], {
    id: 'demo',
    fontFamilies,
    resolveAsset: (path) => {
      if (!assets.includes(path)) throw new ThemePackError('asset', `asset:${path} is missing`);
      return `/packs/demo/${path}`;
    },
  });
}

function cssError(source: string) {
  try {
    compile(source);
  } catch (error) {
    if (error instanceof ThemePackError) return error.detail;
    throw error;
  }
  throw new Error(`accepted: ${source}`);
}

describe('theme pack CSS compiler', () => {
  it('rewrites :theme to the pack root, also with mode variants and inside at-rules', () => {
    const css = compile(`
      :theme { --theme-fg: #111; }
      :theme:root:root .a, :theme[data-theme-effective='dark'] .b > .c { color: red; }
      @media (max-width: 640px) { :theme .d { color: blue; } }
      @supports (display: grid) { :theme .e { display: grid; } }
      :theme .f { &:hover { color: green; } @media (hover: hover) { color: teal; } }
    `);
    expect(css).toContain(`${ROOT} {`);
    expect(css).toContain(`${ROOT}:root:root .a, ${ROOT}[data-theme-effective='dark'] .b > .c {`);
    expect(css).toContain(`@media (max-width: 640px) {\n  ${ROOT} .d {`);
    expect(css).toContain(`@supports (display: grid) {\n  ${ROOT} .e {`);
    expect(css).toContain('&:hover {');
    expect(css).not.toContain(':theme');
  });

  it.each([
    ['a bare selector', '.a { color: red; }'],
    ['one selector of a list', ':theme .a, .b { color: red; }'],
    [':theme further in', '.a :theme { color: red; }'],
    [':theme inside a pseudo-class', ':is(:theme) .a { color: red; }'],
    ['a rule in @media without :theme', '@media screen { body { color: red; } }'],
  ])('requires each top-level selector to start from :theme: %s', (_name, source) => {
    expect(cssError(source)).toMatch(/each selector must start from :theme/);
  });

  it('rewrites asset: URLs and keeps image/font data: URLs', () => {
    const css = compile(`:theme .a { background: url("asset:assets/a.webp"), url(asset:assets/a.webp); mask: image-set("asset:assets/a.webp" 1x); --icon: url('data:image/svg+xml,%3Csvg%3E%3C/svg%3E'); }`);
    expect(css).toContain('background: url("/packs/demo/assets/a.webp"), url("/packs/demo/assets/a.webp");');
    expect(css).toContain('image-set("/packs/demo/assets/a.webp" 1x)');
    expect(css).toContain(`url("data:image/svg+xml,%3Csvg%3E%3C/svg%3E")`);
  });

  it.each([
    ['a remote URL', ':theme { background: url(https://evil.example/x.png); }', /must use asset: or data:/],
    ['a quoted remote URL', ':theme { background: url("https://evil.example/x.png"); }', /must use asset: or data:/],
    ['a protocol-relative URL', ':theme { background: url(//evil.example/x.png); }', /must use asset: or data:/],
    ['a relative URL', ':theme { background: url(../assets/a.webp); }', /must use asset: or data:/],
    ['a host-absolute URL', ':theme { background: url(/icon-192.png); }', /must use asset: or data:/],
    ['an escaped url function', ':theme { background: u\\72l(https://evil.example/x.png); }', /must use asset: or data:/],
    ['a remote image-set', ':theme { background: image-set("https://evil.example/x.png" 1x); }', /must use asset: or data:/],
    ['a src() function', ':theme { background: src("https://evil.example/x.png"); }', /must use asset: or data:/],
    ['a remote URL in a custom property', ':theme { --x: url(https://evil.example/x.png); }', /must use asset: or data:/],
    ['a remote font', '@font-face { font-family: X; src: url(https://evil.example/x.woff2); }', /must use asset: or data:/],
    ['an HTML data: URL', ':theme { background: url("data:text/html,<script>"); }', /data: URLs must be images or fonts/],
    ['a missing asset', ':theme { background: url("asset:assets/missing.webp"); }', /missing/],
    ['an asset path with ..', ':theme { background: url("asset:../secret.webp"); }', /not a pack path/],
    ['a stylesheet as an asset', ':theme { background: url("asset:styles/main.css"); }', /must be an image or font/],
    ['@import', '@import url("https://evil.example/x.css");', /@import is not allowed/],
    ['an escaped @import', '@\\69mport "https://evil.example/x.css";', /escaped at-rule names|@import is not allowed/],
    ['@namespace', '@namespace svg url(http://www.w3.org/2000/svg);', /@namespace is not allowed/],
    ['@layer', '@layer base { :theme { color: red; } }', /@layer is not allowed/],
    ['a nested @font-face', ':theme { @font-face { font-family: X; } }', /@font-face is not allowed inside a rule/],
    ['expression()', ':theme { width: expression(alert(1)); }', /expression\(\) is not allowed/],
    ['a javascript: URL', ':theme { background: url("javascript:alert(1)"); }', /script URL|must use asset: or data:/],
    ['an unquoted javascript: URL', ':theme { background: url(javascript:alert(1)); }', /bad url|script URL/],
    ['a javascript: string', ':theme { --x: "javascript:alert(1)"; }', /script URL/],
    ['-moz-binding', ':theme { -moz-binding: url("asset:assets/a.webp"); }', /-moz-binding is not allowed/],
    ['behavior', ':theme { behavior: url("asset:assets/a.webp"); }', /behavior is not allowed/],
    ['an HTML comment token in a block', ':theme { color: red; <!-- }', /is not allowed|invalid declaration/],
    ['a url() in a selector', ':theme[data-x=url(x)] { color: red; }', /url\(\) is not allowed in a selector/],
    ['an unclosed block', ':theme { color: red;', /unclosed block/],
  ])('rejects %s', (_name, source, message) => {
    expect(cssError(source)).toMatch(message);
  });

  it('names the file and line of a rejection', () => {
    expect(cssError(':theme { color: red; }\n\n:theme { background: url(http://x/y.png); }')).toMatch(/^styles\/main\.css:3: /);
  });

  it('limits block nesting', () => {
    const deep = `:theme ${'.a { '.repeat(30)}color: red;${' }'.repeat(30)}`;
    expect(cssError(deep)).toMatch(/nested too deeply/);
  });

  it('prefixes keyframes and their references, but not other idents', () => {
    const css = compile(`
      @keyframes hop { from { transform: none; } to { transform: translateY(-2px); } }
      @keyframes "quoted name" { to { opacity: 1; } }
      :theme .a { animation: hop 700ms ease, "quoted name" 1s; --motion: hop 1s; transition: hop 1s; }
      :theme .b { animation-name: hop; }
    `);
    expect(css).toContain('@keyframes demo--hop {');
    expect(css).toContain('@keyframes "demo--quoted name" {');
    expect(css).toContain('animation: demo--hop 700ms ease, "demo--quoted name" 1s;');
    expect(css).toContain('--motion: demo--hop 1s;');
    expect(css).toContain('transition: hop 1s;');
    expect(css).toContain('animation-name: demo--hop;');
  });

  it('prefixes @font-face families and the manifest fonts wherever they are used', () => {
    const css = compile(
      `
      @font-face { font-family: "Pocket Round"; src: url("asset:assets/f.woff2") format("woff2"); font-display: swap; }
      :theme { --font-ui: "Nunito", Pocket Round, sans-serif; }
      :theme .a { font-family: 'Pocket Round', system-ui; font: 600 14px/1.2 Nunito, sans-serif; }
      :theme .b { font-family: Arial, sans-serif; }
    `,
      undefined,
      ['Nunito'],
    );
    expect(css).toContain('font-family: "demo--Pocket Round";');
    expect(css).toContain('src: url("/packs/demo/assets/f.woff2") format("woff2");');
    expect(css).toContain('--font-ui: "demo--Nunito", "demo--Pocket Round", sans-serif;');
    expect(css).toContain('font-family: "demo--Pocket Round", system-ui;');
    expect(css).toContain('font: 600 14px/1.2 "demo--Nunito", sans-serif;');
    expect(css).toContain('font-family: Arial, sans-serif;');
  });

  it('keeps keyframes and font names from every stylesheet of the pack', () => {
    const css = compileStylesheets(
      [
        { file: 'styles/a.css', source: ':theme .a { animation: spin 1s; }' },
        { file: 'styles/b.css', source: '@keyframes spin { to { rotate: 1turn; } }' },
      ],
      { id: 'demo', resolveAsset: () => '' },
    );
    expect(css).toContain('animation: demo--spin 1s;');
    expect(css.indexOf('/* styles/a.css */')).toBeLessThan(css.indexOf('/* styles/b.css */'));
  });

  it('ignores top-level HTML comment tokens like browsers do', () => {
    expect(compile('<!-- :theme { color: red; } -->')).not.toMatch(/<!--|-->/);
  });

  it('drops @charset and comments', () => {
    const css = compile('@charset "utf-8";\n/* </style><script>alert(1)</script> */\n:theme { color: red; }');
    expect(css).not.toContain('@charset');
    expect(css).not.toContain('<script>');
  });
});
