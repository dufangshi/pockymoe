import { expect, it } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { browserLabel } from './browserLabel';

it.each([
  ['Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/140.0.0.0 Safari/537.36', 'Chrome on macOS'],
  ['Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/140.0.0.0 Safari/537.36 Edg/140.0.0.0', 'Edge on Windows'],
  ['Mozilla/5.0 (X11; Linux x86_64) Gecko/20100101 Firefox/140.0', 'Firefox on Linux'],
  ['Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Version/18.6 Safari/605.1.15', 'Safari on macOS'],
  ['Mozilla/5.0 (Linux; Android 15; Pixel 9) Chrome/140.0.0.0 Mobile Safari/537.36', 'Chrome on Android'],
  ['Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 CriOS/140.0 Mobile/15E148 Safari/604.1', 'Chrome on iOS'],
  ['Mozilla/5.0 (iPad; CPU OS 18_6 like Mac OS X) FxiOS/140.0 Mobile/15E148 Safari/605.1.15', 'Firefox on iOS'],
  ['Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15) Version/18.6 Mobile/15E148 Safari/604.1', 'Safari on iOS'],
  ['Mozilla/5.0 (X11; CrOS x86_64 16222.0.0) Chrome/140.0.0.0 Safari/537.36', 'Chrome on ChromeOS'],
  ['Mozilla/5.0 (Windows NT 10.0) Chrome/140.0 Safari/537.36 OPR/120.0', 'Opera on Windows'],
  ['unrecognized user agent', 'Browser'],
])('labels %s as %s', (userAgent, label) => {
  setLocale('en'); expect(browserLabel(userAgent)).toBe(label);
});
it('localizes the label without exposing an unknown user agent', () => {
  setLocale('zh-CN');
  expect(browserLabel('Mozilla/5.0 (Windows NT 10.0) Chrome/140.0 Safari/537.36')).toBe('Windows 上的 Chrome');
  expect(browserLabel('unrecognized user agent')).toBe('浏览器');
  setLocale('en');
});
