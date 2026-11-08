import { expect, it } from 'vitest';
import { fileChange, parsePatch } from './patch';

it('splits a multi-file patch, counts lines and keeps the text unchanged', () => {
  const patch = [
    'diff --git a/src/app.rs b/src/app.rs', 'index 1..2 100644', '--- a/src/app.rs', '+++ b/src/app.rs',
    '@@ -1,2 +1,2 @@', ' keep', '-old', '+new', '+--- not a header',
    'diff --git a/new.txt b/new.txt', 'new file mode 100644', '--- /dev/null', '+++ b/new.txt', '@@ -0,0 +1 @@', '+hello',
    'diff --git a/img.png b/img.png', 'Binary files a/img.png and b/img.png differ', '',
  ].join('\n');
  const { preamble, files } = parsePatch(patch);
  expect(preamble).toEqual([]);
  expect(files.map(file => [file.path, file.additions, file.deletions, file.created, file.binary])).toEqual([
    ['src/app.rs', 2, 1, false, false], ['new.txt', 1, 0, true, false], ['img.png', 0, 0, false, true],
  ]);
  expect(files[0]!.lines.map(line => line.kind)).toEqual(['hunk', 'ctx', 'del', 'add', 'add']);
  expect(files[0]!.lines.at(-1)!.text).toBe('+--- not a header');
});

it('shows text without a git header as-is and maps porcelain codes', () => {
  expect(parsePatch('untracked contents\nsecond').preamble.map(line => line.text)).toEqual(['untracked contents', 'second']);
  expect(fileChange('?')).toBe('untracked');
  expect(fileChange('M')).toBe('modified');
  expect(fileChange('U', 'U')).toBe('conflicted');
  expect(fileChange('A', 'A')).toBe('conflicted');
});

it('decodes Git C-quoted UTF-8 paths without changing patch text', () => {
  const name = String.raw`\344\270\255\346\226\207.txt`;
  const patch = [
    `diff --git "a/${name}" "b/${name}"`, `--- "a/${name}"`, `+++ "b/${name}"`,
    '@@ -1 +1 @@', '-old', '+new',
    String.raw`diff --git "a/old\t\"\\name.png" "b/new\t\"\\name.png"`,
    'similarity index 100%', String.raw`rename from "old\t\"\\name.png"`, String.raw`rename to "new\t\"\\name.png"`,
  ].join('\n');
  const files = parsePatch(patch).files;
  expect(files[0]!.path).toBe('中文.txt');
  expect(files[0]!.oldPath).toBeUndefined();
  expect(files[0]!.lines.map(line => line.text)).toEqual(['@@ -1 +1 @@', '-old', '+new']);
  expect(files[1]!.oldPath).toBe('old\t"\\name.png');
  expect(files[1]!.path).toBe('new\t"\\name.png');
  expect(files[1]!.lines.at(-1)!.text).toBe(String.raw`rename to "new\t\"\\name.png"`);
});
