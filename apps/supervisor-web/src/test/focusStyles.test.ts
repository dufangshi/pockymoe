// @vitest-environment node
import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';

const root = path.resolve(import.meta.dirname, '../../../..');
const roots = ['apps/supervisor-web/src', 'pockymoe-thread-ui/packages/thread-ui/src', 'themes'];

function files(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const name = path.join(directory, entry.name);
    return entry.isDirectory() ? files(name) : [name];
  });
}

function unsafeCss(source: string) {
  const css = source.replace(/\/\*[\s\S]*?\*\//g, '');
  // These two classes are native text inputs. Everything else must name its
  // typing element explicitly, including inside :has().
  const textTarget = /(?:\b(?:input|textarea|select)|\[contenteditable=["']true["']\]|\.thread-goal-compose-input|\.thread-graph-create-thread-input|:where\(input,\s*textarea,\s*select\))$/;
  return [...css.matchAll(/:focus(?:-within)?(?![\w-])/g)]
    .filter(match => match[0] === ':focus-within' || !textTarget.test(css.slice(0, match.index)))
    .map(match => css.slice(Math.max(0, css.lastIndexOf('}', match.index) + 1), match.index + match[0].length).trim());
}

function unguardedHover(source: string) {
  const ancestors: string[] = [];
  const failures: string[] = [];
  const css = source.replace(/\/\*[\s\S]*?\*\//g, '');
  for (const match of css.matchAll(/([^{};]*)\{|([{};])/g)) {
    if (match[1] !== undefined) {
      const prelude = match[1].trim();
      const hoverMedia = /^@media\b(?!.*\b(?:not|or)\b)(?!.*,)[^{}]*\(hover:\s*hover\)/;
      if (/:hover\b/.test(prelude) && !ancestors.some(value => hoverMedia.test(value))) failures.push(prelude);
      ancestors.push(prelude);
    } else if (match[2] === '}') ancestors.pop();
  }
  return failures;
}

describe('pointer clicks leave no focus styling behind', () => {
  it('rejects reveal/accent selectors while allowing explicit typing focus', () => {
    expect(unsafeCss('.row:focus-within .copy { opacity: 1 } button:focus { color: red }')).toHaveLength(2);
    expect(unsafeCss('.row:has(:focus-visible) .copy {} button:active {} button:focus-visible {} input:focus {} .composer:has([contenteditable="true"]:focus) {}')).toEqual([]);
  });

  it('rejects sticky touch hover, including mixed state selectors', () => {
    expect(unguardedHover('button:hover, button:focus-visible { color: red } @media (max-width: 639px) { .row:is(:hover, [data-open]) { color: red } }')).toHaveLength(2);
    expect(unguardedHover('@media not (hover: hover) { button:hover {} } @media (hover: hover), (pointer: coarse) { button:hover {} }')).toHaveLength(2);
    expect(unguardedHover('button:active {} button:focus-visible {} @media (hover: hover) { button:hover {} @media (min-width: 640px) { .row:has(:hover) {} } }')).toEqual([]);
  });

  it.each(roots)('%s limits hover styling to pointers that hover', directory => {
    const failures = files(path.join(root, directory)).filter(name => name.endsWith('.css')).flatMap(name =>
      unguardedHover(readFileSync(name, 'utf8')).map(selector => `${path.relative(root, name)}: ${selector}`),
    );
    expect(failures).toEqual([]);
  });

  it.each(roots)('%s uses keyboard focus for controls and hover affordances', directory => {
    const failures: string[] = [];
    for (const name of files(path.join(root, directory))) {
      if (!/\.(?:css|tsx?)$/.test(name) || /\.(?:test|spec)\./.test(name)) continue;
      const source = readFileSync(name, 'utf8');
      const relative = path.relative(root, name);
      if (name.endsWith('.css')) {
        failures.push(...unsafeCss(source).map(selector => `${relative}: ${selector}`));
      } else if (/\.tsx?$/.test(name)) {
        const file = ts.createSourceFile(name, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
        const visit = (node: ts.Node) => {
          if (ts.isJsxAttribute(node) && node.name.getText(file) === 'className') {
            const element = node.parent.parent;
            const tag = ts.isJsxOpeningElement(element) || ts.isJsxSelfClosingElement(element) ? element.tagName.getText(file) : '';
            if (!['input', 'textarea', 'select'].includes(tag) && /\bfocus(?:-within)?:/.test(node.getText(file))) {
              failures.push(`${relative}: ${node.getText(file)}`);
            }
          }
          ts.forEachChild(node, visit);
        };
        visit(file);
      }
    }
    expect(failures).toEqual([]);
  });
});
