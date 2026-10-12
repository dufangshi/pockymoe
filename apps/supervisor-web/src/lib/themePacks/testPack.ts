/* Small valid packs for the theme pack tests. */

const encoder = new TextEncoder();

/** The first bytes of each binary type, enough for the signature check. */
export const WEBP = new Uint8Array([...encoder.encode('RIFF'), 0, 0, 0, 0, ...encoder.encode('WEBP'), 1, 2, 3]);
export const WOFF2 = new Uint8Array([...encoder.encode('wOF2'), 0, 1, 2, 3]);
export const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

export function testManifest(overrides: Record<string, unknown> = {}) {
  return {
    schema: 1,
    id: 'test-pack',
    name: { en: 'Test Pack', 'zh-CN': '测试' },
    version: '1.0.0',
    author: 'Tests',
    swatches: ['#112233', '#ffffff'],
    chrome: { light: '#112233', dark: '#000000' },
    styles: ['styles/main.css'],
    fonts: [{ family: 'Test Sans', src: 'assets/test.woff2', weight: '400 700' }],
    agents: { default: { avatar: 'assets/agent.webp' }, claude: { avatar: 'assets/claude.webp', peek: 'assets/claude.webp' } },
    features: ['composer.richMenus', 'future.flag'],
    ...overrides,
  };
}

export const TEST_CSS = `:theme { --theme-accent-solid: #123456; }
:theme[data-theme-effective='dark'] .thread-ui-shell { color: red; }
:theme .brand { background: url("asset:assets/agent.webp") center / cover; font-family: "Test Sans", sans-serif; animation: wiggle 1s; }
@keyframes wiggle { from { opacity: 0; } to { opacity: 1; } }
`;

export function testPackFiles(options: { manifest?: Record<string, unknown>; css?: string; extra?: [string, Uint8Array][] } = {}): [string, Uint8Array][] {
  return [
    ['theme.json', encoder.encode(JSON.stringify(options.manifest ?? testManifest()))],
    ['styles/main.css', encoder.encode(options.css ?? TEST_CSS)],
    ['assets/agent.webp', WEBP],
    ['assets/claude.webp', WEBP],
    ['assets/test.woff2', WOFF2],
    ['LICENSE', encoder.encode('MIT')],
    ...(options.extra ?? []),
  ];
}

export const text = (value: string) => encoder.encode(value);
