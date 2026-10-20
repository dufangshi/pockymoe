import { act, renderHook } from '@testing-library/react';
import { beforeEach, expect, it } from 'vitest';
import { resetKeptDrafts, useThreadDrafts } from './useThreadDrafts';
beforeEach(() => resetKeptDrafts());
it('keeps drafts by device and thread and ignores a departed composer callback', () => {
  const { result, rerender } = renderHook(
    ({ source }) => useThreadDrafts(source),
    { initialProps: { source: 'device:a' } },
  );
  act(() => result.current[1]({ prompt: 'A only', attachments: [] }));
  const stale = result.current[1];
  rerender({ source: 'device:b' });
  expect(result.current[0].prompt).toBe('');
  act(() => {
    stale({ prompt: 'late A', attachments: [] });
    result.current[1]({ prompt: 'B only', attachments: [] });
  });
  rerender({ source: 'device:a' });
  expect(result.current[0].prompt).toBe('late A');
  act(() => stale({ prompt: 'older A callback', attachments: [] }));
  expect(result.current[0].prompt).toBe('late A');
  rerender({ source: 'other-device:a' });
  expect(result.current[0].prompt).toBe('');
});

it('shares both pane drafts including attachments across swaps and rejects stale pane callbacks', () => {
  const { result, rerender } = renderHook(({ a, b }) => useThreadDrafts(a, b), { initialProps: { a: 'dev:a', b: 'dev:b' } });
  const file = { file: new File(['review'], 'review.txt'), kind: 'file' as const, clientId: 'b-file', originalName: 'review.txt', placeholder: '[FILE review.txt]' };
  act(() => {
    result.current[1]({ prompt: 'A', attachments: [] });
    result.current[3]({ prompt: 'B [FILE review.txt]', attachments: [file] });
  });
  const stale = result.current[3];
  rerender({ a: 'dev:b', b: 'dev:a' });
  expect(result.current[0].attachments).toEqual([file]);
  expect(result.current[2].prompt).toBe('A');
  act(() => stale({ prompt: '', attachments: [] }));
  expect(result.current[0].prompt).toBe('B [FILE review.txt]');
  rerender({ a: 'dev:b', b: 'dev:c' });
  expect(result.current[2].prompt).toBe('');
  rerender({ a: 'dev:b', b: 'dev:a' });
  expect(result.current[2].prompt).toBe('A');
});


it('keeps a draft when the page that edits it remounts (another device, another route)', () => {
  const first = renderHook(() => useThreadDrafts('dev:a'));
  act(() => first.result.current[1]({ prompt: 'half written', attachments: [] }));
  first.unmount();
  const second = renderHook(() => useThreadDrafts('dev:a'));
  expect(second.result.current[0].prompt).toBe('half written');
  // An emptied draft is forgotten, not kept as blank.
  act(() => second.result.current[1]({ prompt: '', attachments: [] }));
  second.unmount();
  expect(renderHook(() => useThreadDrafts('dev:a')).result.current[0].prompt).toBe('');
});
