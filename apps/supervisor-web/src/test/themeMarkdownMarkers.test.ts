import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';

// A theme pack changes looks, never meaning (docs/theme-packs.md): one that hides
// an answer's list markers must draw its own, or numbered steps lose their numbers.
const themes = path.resolve(import.meta.dirname, '../../../../themes');
const css = (theme: string) => readdirSync(path.join(themes, theme, 'styles'))
  .filter((file) => file.endsWith('.css'))
  .map((file) => readFileSync(path.join(themes, theme, 'styles', file), 'utf8'))
  .join('\n')
  .replace(/\/\*[\s\S]*?\*\//g, '');

describe('theme packs keep answer list markers', () => {
  for (const theme of readdirSync(themes).filter((entry) => !entry.startsWith('.') && readdirSync(path.join(themes, entry)).includes('styles'))) {
    it(theme, () => {
      const source = css(theme);
      const rules = [...source.matchAll(/([^{}]+)\{([^{}]*)\}/g)].filter(([, head]) => head!.includes('.thread-graph-markdown'));
      const hides = (tag: 'ul' | 'ol') => rules.some(([, head, body]) =>
        new RegExp(`\\b${tag}\\b`).test(head!) && !/::?before|li\b/.test(head!) && /list-style:\s*none/.test(body!));
      const draws = (tag: 'ul' | 'ol') => rules.some(([, head, body]) =>
        new RegExp(`\\b${tag}\\s*>\\s*li::before`).test(head!) && /content:\s*(counter|['"])/.test(body!));
      for (const tag of ['ul', 'ol'] as const) {
        if (hides(tag)) expect(draws(tag), `${theme} hides ${tag} markers without drawing its own`).toBe(true);
      }
    });
  }
});
