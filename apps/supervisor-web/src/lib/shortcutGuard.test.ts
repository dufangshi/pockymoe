import { readdirSync, readFileSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import ts from 'typescript';
import { expect, it } from 'vitest';

/**
 * docs/ui-principles.md §19: a shortcut's command modifier is ⌘ on a Mac and
 * Ctrl elsewhere, decided by the shared `matchesModKey`. A bare
 * `metaKey || ctrlKey` takes both on every platform, so on a Mac it steals
 * Ctrl+F, Ctrl+K and the other text-editing and terminal keys.
 *
 * "Bare" is a test of exactly those two flags, in either order, on an event or
 * as plain variables: `a.metaKey || a.ctrlKey`, and its mirror
 * `!a.metaKey && !a.ctrlKey`. A longer chain that also reads Alt, Shift, the
 * button or the key (`metaKey || ctrlKey || shiftKey || altKey`: "leave a
 * modified click to the browser") is not a shortcut's modifier test and passes.
 */
const DELIBERATE: Record<string, string> = {
  // The composer's send key takes Ctrl or ⌘ on every platform
  // (docs/account-composer-shortcuts.md): `matchesSendModKey` is the one place
  // that says so, and the composer and the new-thread form call it.
  'pockymoe-thread-ui/packages/thread-ui/src/components/shortcuts.ts': 'matchesSendModKey',
};

function flagName(node: ts.Expression, negated: boolean): string | null {
  while (ts.isParenthesizedExpression(node)) node = node.expression;
  if (negated) {
    if (!ts.isPrefixUnaryExpression(node) || node.operator !== ts.SyntaxKind.ExclamationToken) return null;
    node = node.operand;
    while (ts.isParenthesizedExpression(node)) node = node.expression;
  }
  const name = ts.isIdentifier(node) ? node.text : ts.isPropertyAccessExpression(node) ? node.name.text : null;
  return name === 'metaKey' || name === 'ctrlKey' ? name : null;
}
function bareModifierPairs(source: string, filename = 'keys.tsx') {
  const ast = ts.createSourceFile(filename, source, ts.ScriptTarget.Latest, true);
  const found: number[] = [];
  function visit(node: ts.Node) {
    if (ts.isBinaryExpression(node)) {
      const or = node.operatorToken.kind === ts.SyntaxKind.BarBarToken;
      const and = node.operatorToken.kind === ts.SyntaxKind.AmpersandAmpersandToken;
      // Part of a longer chain of the same operator: the whole chain is judged, not this pair.
      const inChain = ts.isBinaryExpression(node.parent) && node.parent.operatorToken.kind === node.operatorToken.kind;
      if ((or || and) && !inChain) {
        const names = [flagName(node.left, and), flagName(node.right, and)];
        if (names.includes('metaKey') && names.includes('ctrlKey')) found.push(ast.getLineAndCharacterOfPosition(node.getStart(ast)).line + 1);
      }
    }
    ts.forEachChild(node, visit);
  }
  visit(ast); return found;
}

it('recognizes a bare metaKey/ctrlKey pair and leaves longer modifier tests and comments alone', () => {
  expect(bareModifierPairs([
    'if (event.metaKey || event.ctrlKey) open();',
    'if (!(event.ctrlKey || event.metaKey) || event.altKey) return;',
    'const modifier = metaKey || ctrlKey;',
    "if ((input?.metaKey || input?.ctrlKey) && input.key === 'f') filter();",
    "if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) send();",
    'if (!event.metaKey && !event.ctrlKey) return;',
  ].join('\n'))).toEqual([1, 2, 3, 4, 5, 6]);
  expect(bareModifierPairs([
    '// event.metaKey || event.ctrlKey',
    'if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;',
    'if (event.button !== 0 || event.metaKey || event.ctrlKey) return;',
    'if (!event.metaKey && !event.ctrlKey && !event.altKey && event.key.length === 1) type();',
    'if (matchesModKey(event) && !event.altKey) open();',
    'if (isMacPlatform() ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey) open();',
    'const text = "metaKey || ctrlKey";',
  ].join('\n'))).toEqual([]);
});
it('keeps Web and shared UI shortcuts on the platform modifier (matchesModKey), not metaKey || ctrlKey', () => {
  const repo = resolve('../..');
  const violations: string[] = [];
  const exceptions = new Set<string>();
  for (const root of [resolve('src'), resolve('../../pockymoe-thread-ui/packages/thread-ui/src')]) {
    function scan(dir: string) {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const path = join(dir, entry.name);
        if (entry.isDirectory()) scan(path);
        else if (/\.tsx?$/.test(path) && !/\.(?:test|spec)\./.test(path)) {
          const lines = bareModifierPairs(readFileSync(path, 'utf8'), path);
          const file = relative(repo, path).split('\\').join('/');
          if (file in DELIBERATE) { if (lines.length) exceptions.add(file); } else for (const line of lines) violations.push(`${file}:${line}`);
        }
      }
    }
    scan(root);
  }
  expect(violations).toEqual([]);
  // An exception that no longer applies leaves the list.
  expect([...exceptions].sort()).toEqual(Object.keys(DELIBERATE).sort());
});
