import { expect, it } from 'vitest';
import { fileChange, parsePatch } from './patch';

it('splits a multi-file patch, counts lines and keeps the text unchanged', () => {
  const patch = [
    'diff --git a/src/app.rs b/src/app.rs', 'index 1..2 100644', '--- a/src/app.rs', '+++ b/src/app.rs',
    '@@ -1,2 +1,3 @@', ' keep', '-old', '+new', '+--- not a header',
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

it('opens a file at a bare ---/+++ header instead of showing it as deleted and added lines', () => {
  const parsed = parsePatch('--- /dev/null\n+++ b/NEW-untracked.txt\n@@ -0,0 +1 @@\n+untracked new file\n');
  expect(parsed.preamble).toEqual([]);
  expect(parsed.files).toHaveLength(1);
  expect(parsed.files[0]).toMatchObject({ path: 'NEW-untracked.txt', created: true, additions: 1, deletions: 0 });
  expect(parsed.files[0]!.lines.map(line => line.kind)).toEqual(['hunk', 'add']);
});

it('keeps an untracked text file after an untracked binary one under its own name', () => {
  // The Supervisor's untracked patches have no `diff --git` line, and a binary
  // one has no hunk: the next file's header still opens a file of its own.
  const parsed = parsePatch([
    'diff --git a/src/app.rs b/src/app.rs', 'index 1..2 100644', '--- a/src/app.rs', '+++ b/src/app.rs', '@@ -1 +1 @@', '-old', '+new',
    '--- /dev/null', '+++ b/docs/diagram.png', 'Binary files /dev/null and b/docs/diagram.png differ',
    '--- /dev/null', '+++ b/sup2.pid', '@@ -0,0 +1 @@', '+1087690',
    '--- /dev/null', '+++ b/retry.toml', '@@ -0,0 +1,2 @@', '+-- a removed-looking line', '+++ an added-looking line',
  ].join('\n'));
  expect(parsed.files.map(file => [file.path, file.created, file.binary, file.additions])).toEqual([
    ['src/app.rs', false, false, 1], ['docs/diagram.png', true, true, 0], ['sup2.pid', true, false, 1], ['retry.toml', true, false, 2],
  ]);
  expect(parsed.files[1]!.lines).toEqual([{ kind: 'meta', text: 'Binary files /dev/null and b/docs/diagram.png differ' }]);
  expect(parsed.files[2]!.lines.map(line => line.text)).toEqual(['@@ -0,0 +1 @@', '+1087690']);
  expect(parsed.files[3]!.lines.map(line => line.kind)).toEqual(['hunk', 'add', 'add']);
});

it('reads a hunk by its line counts, so header-like content stays content', () => {
  const parsed = parsePatch([
    'diff --git a/notes.md b/notes.md', '--- a/notes.md', '+++ b/notes.md', '@@ -1,2 +1,2 @@', '--- old rule', '+++ new rule', ' end',
    '\\ No newline at end of file',
    'diff --git a/old.txt b/new.txt', 'similarity index 100%', 'rename from old.txt', 'rename to new.txt',
    'diff --git a/run.sh b/run.sh', 'old mode 100644', 'new mode 100755',
  ].join('\n'));
  expect(parsed.files.map(file => file.path)).toEqual(['notes.md', 'new.txt', 'run.sh']);
  expect(parsed.files[0]!.lines.map(line => line.kind)).toEqual(['hunk', 'del', 'add', 'ctx', 'note']);
  expect(parsed.files[1]!.lines.every(line => line.kind === 'meta')).toBe(true);
  expect(parsed.files[1]!.oldPath).toBe('old.txt');
  expect(parsed.files[2]!.lines.map(line => line.kind)).toEqual(['note', 'note']);
});
