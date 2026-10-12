import { translate } from '@pockymoe/thread-ui/i18n';

/** Cosmetic device-access label: never upload the full user agent. */
export function browserLabel(userAgent = navigator.userAgent) {
  const browser = /Edg(?:e|A|iOS)?\//.test(userAgent) ? 'Edge'
    : /(?:OPR|Opera)\//.test(userAgent) ? 'Opera'
    : /(?:Firefox|FxiOS)\//.test(userAgent) ? 'Firefox'
    : /(?:Chrome|CriOS)\//.test(userAgent) ? 'Chrome'
    : /Safari\//.test(userAgent) ? 'Safari'
    : translate('auth.accountBrowserUnknown');
  const platform = /(?:iPhone|iPad|iPod)|Macintosh.*Mobile\//.test(userAgent) ? 'iOS'
    : /Android/.test(userAgent) ? 'Android'
    : /Windows/.test(userAgent) ? 'Windows'
    : /CrOS/.test(userAgent) ? 'ChromeOS'
    : /(?:Macintosh|Mac OS X)/.test(userAgent) ? 'macOS'
    : /Linux/.test(userAgent) ? 'Linux' : '';
  return platform ? translate('auth.accountBrowserLabel', { value1: browser, value2: platform }) : browser;
}
