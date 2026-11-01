import { readdirSync, readFileSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import ts from 'typescript';
import { expect, it } from 'vitest';

function nativeSelects(source: string, filename = 'control.tsx') {
  const ast = ts.createSourceFile(filename, source, ts.ScriptTarget.Latest, true);
  const found: number[] = [];
  function visit(node: ts.Node) {
    const jsx = (ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) && node.tagName.getText(ast) === 'select';
    const call = ts.isCallExpression(node) && node.arguments[0] && ts.isStringLiteral(node.arguments[0]) && node.arguments[0].text === 'select' &&
      (ts.isIdentifier(node.expression) ? node.expression.text === 'createElement' : ts.isPropertyAccessExpression(node.expression) && node.expression.name.text === 'createElement');
    if (jsx || call) found.push(ast.getLineAndCharacterOfPosition(node.getStart(ast)).line + 1);
    ts.forEachChild(node, visit);
  }
  visit(ast); return found;
}
it('recognizes native select creation without mistaking comments or custom components for controls', () => {
  expect(nativeSelects('<select><option>A</option></select>; document.createElement("select"); React.createElement("select");')).toEqual([1, 1, 1]);
  expect(nativeSelects('// <select>\n<Select options={options} onValueChange={choose} />; const example = "<select>";')).toEqual([]);
});
it('keeps Web and shared UI on the shared Select rather than native select controls', () => {
  const violations: string[] = [];
  for (const root of [resolve('src'), resolve('../../pockymoe-thread-ui/packages/thread-ui/src')]) {
    function scan(dir: string) {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const path = join(dir, entry.name);
        if (entry.isDirectory()) scan(path);
        else if (/\.tsx?$/.test(path) && !/\.(?:test|spec)\./.test(path)) {
          for (const line of nativeSelects(readFileSync(path, 'utf8'), path)) violations.push(`${relative(root, path)}:${line}`);
        }
      }
    }
    scan(root);
  }
  expect(violations).toEqual([]);
});
