// Run with the normal Web unit suite. Scan expressions rather than source text
// so comments, regexes and imports cannot hide or spuriously trigger the guard.
import { readdirSync, readFileSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';

function rootUrls(source: string, filename = 'example.tsx') {
  const ast = ts.createSourceFile(filename, source, ts.ScriptTarget.Latest, true);
  const found: string[] = [];
  function visit(node: ts.Node) {
    const text = ts.isTemplateExpression(node) ? node.head.text
      : ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node) ? node.text : '';
    if (/^\/(?:api|ws|relay)(?:\/|$|\?)/.test(text)) {
      // These compare Supervisor wire paths; they never construct browser URLs.
      const parent = node.parent;
      const predicate = ts.isCallExpression(parent) && ts.isPropertyAccessExpression(parent.expression) &&
        parent.expression.name.text === 'startsWith' && ['/api/', '/relay/'].includes(text);
      if (!predicate) found.push(node.getText(ast));
    }
    ts.forEachChild(node, visit);
  }
  visit(ast);
  return found;
}

describe('browser URL guard', () => {
  it('rejects literals, templates and indirect URL construction', () => {
    expect(rootUrls("fetch('/api/version'); new WebSocket(`/ws?x=${id}`); const prefix = '/relay/devices/';")).toEqual(["'/api/version'", '`/ws?x=${id}`', "'/relay/devices/'"]);
    expect(rootUrls("request('api/version'); relayUrl(`relay/devices/${id}/api/version`); const p = supervisorPath('api/version'); // /api/comment\nconst route = '/relay-portal'; const re = /^\\/api/;")).toEqual([]);
  });
  it('prevents new root-absolute API, WebSocket and relay URLs in src (excluding tests)', () => {
    const root = resolve('src');
    const violations: string[] = [];
    function scan(dir: string) {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const path = join(dir, entry.name);
        if (entry.isDirectory()) scan(path);
        else if (/\.tsx?$/.test(path) && !/\.(?:test|spec)\./.test(path)) {
          for (const literal of rootUrls(readFileSync(path, 'utf8'), path)) {
            violations.push(`${relative(root, path)}: ${literal}`);
          }
        }
      }
    }
    scan(root);
    expect(violations).toEqual([]);
  });
});
