// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from 'vitest';
import { applyFontSize, FONT_SIZE_KEY, normalizeFontSize, readFontSize, setFontSize } from './fontSize';

describe('font size preference', () => {
  beforeEach(() => localStorage.clear());
  it('rejects invalid, out-of-range and missing storage values', () => {
    for (const value of [null, '', 'NaN', 0, 100, -1, undefined]) expect(normalizeFontSize(value)).toBe(16);
    expect(normalizeFontSize('18')).toBe(18);
  });
  it('scales the conversation (not the page) immediately and on a later page load', () => {
    const zoom = () => document.documentElement.style.getPropertyValue('--chat-zoom');
    document.documentElement.style.fontSize = '20px';
    setFontSize(20);
    expect(localStorage.getItem(FONT_SIZE_KEY)).toBe('20');
    expect(zoom()).toBe('1.25');
    // The page's own text size is left to the browser, so settings and buttons keep theirs.
    expect(document.documentElement.style.fontSize).toBe('');
    document.documentElement.style.removeProperty('--chat-zoom');
    applyFontSize();
    expect(zoom()).toBe('1.25');
    expect(readFontSize()).toBe(20);
    setFontSize(16);
    expect(zoom()).toBe('1');
  });
});
