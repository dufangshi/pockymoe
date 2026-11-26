import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { hunkPatches, ReviewPatch } from './ReviewPatch';
import { parsePatch } from './patch';
const patch = 'diff --git a/a.rs b/a.rs\nindex abc..def 100644\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n@@ -10 +10 @@\n-before\n+after\n';
it('preserves exact headers and isolates each complete hunk for Git apply', () => {
  const hunks = hunkPatches(patch).get('a.rs')!;
  expect(hunks).toHaveLength(2);
  expect(hunks[0]).toContain('index abc..def 100644\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n');
  expect(hunks[0]).not.toContain('before');
  expect(hunks[1]).not.toContain('+new');
});
it('find opens collapsed files and hunk writes contain only that hunk', () => {
  setLocale('en'); const write = vi.fn().mockResolvedValue({});
  render(<ReviewPatch patch={patch} parsed={parsePatch(patch)} single={false} onSelect={() => {}} split={false} collapsed area="worktree" write={write} busy={false} />);
  expect(screen.queryByText('+after')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Find in diff' }));
  fireEvent.change(screen.getByRole('textbox', { name: 'Find in diff' }), { target: { value: 'after' } });
  expect(screen.getByText('+after')).toHaveAttribute('data-match', 'true');
  fireEvent.click(screen.getAllByRole('button', { name: 'Stage Hunk' })[1]!);
  expect(write).toHaveBeenCalledWith('git/stage', { patch: hunkPatches(patch).get('a.rs')![1] });
});
it('addresses Git-quoted UTF-8 paths without changing patch bytes', () => {
  const quoted = 'diff --git "a/\\303\\251.rs" "b/\\303\\251.rs"\n--- "a/\\303\\251.rs"\n+++ "b/\\303\\251.rs"\n@@ -1 +1 @@\n-a\n+b\n';
  expect(hunkPatches(quoted).get('é.rs')).toEqual([quoted]);
});
it('keeps selected lines when pointer events bubble from the comment dialog', () => {
  setLocale('en'); const insert = vi.fn().mockReturnValue(true);
  render(<ReviewPatch patch={patch} parsed={parsePatch(patch)} single={false} onSelect={() => {}} split={false} collapsed={false} area="turn" write={undefined} busy={false} onReviewQuote={insert} />);
  const line = screen.getByText('+new');
  const selection = window.getSelection()!; const range = document.createRange(); range.selectNodeContents(line); selection.removeAllRanges(); selection.addRange(range);
  fireEvent.mouseUp(line);
  fireEvent.click(screen.getByRole('button', { name: 'Comment' }));
  selection.removeAllRanges();
  const comment = screen.getByRole('textbox', { name: 'Comment' });
  fireEvent.mouseUp(comment);
  fireEvent.change(comment, { target: { value: 'Handle malformed input.' } });
  const add = screen.getByRole('button', { name: 'Add comment' });
  fireEvent.mouseUp(add); fireEvent.click(add);
  expect(insert).toHaveBeenCalledWith(expect.objectContaining({ path: 'a.rs', startLine: 1, endLine: 1, text: 'new', comment: 'Handle malformed input.' }));
});
