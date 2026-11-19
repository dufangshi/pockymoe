import { act, renderHook } from '@testing-library/react';
import { beforeEach, expect, it } from 'vitest';
import { getThreadDraftPreview, resetKeptDrafts, seedThreadDraft, takeThreadAutoSend, useThreadDrafts } from './useThreadDrafts';
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

it('counts unsent conversations and attachments, including retained and attachment-only drafts', async () => {
  const { getUnsentThreadDraftCounts, getThreadDraftRevision } = await import('./useThreadDrafts');
  const before = getThreadDraftRevision();
  const hook = renderHook(() => useThreadDrafts('text', 'file'));
  const attachment = { file: new File(['draft'], 'draft.txt'), kind: 'file' as const, clientId: 'draft', originalName: 'draft.txt', placeholder: '[FILE draft.txt]' };
  act(() => { hook.result.current[1]({ prompt: 'Unsent', attachments: [] }); hook.result.current[3]({ prompt: '', attachments: [attachment] }); });
  expect(getUnsentThreadDraftCounts()).toEqual({ drafts: 2, attachments: 1 }); expect(getThreadDraftRevision()).toBeGreaterThan(before);
  hook.unmount(); expect(getUnsentThreadDraftCounts()).toEqual({ drafts: 2, attachments: 1 });
});

it('retains actual titles with pending primary/reference drafts without persisting content', async () => {
  const { useThreadDraftTitle, getUnsentThreadDraftTitles, getUnsentThreadDraftCounts, registerThreadDraftTitle } = await import('./useThreadDrafts');
  const hook = renderHook(() => { useThreadDraftTitle('device:a', 'Fix login'); useThreadDraftTitle('other:b', 'Review attachment'); return useThreadDrafts('device:a', 'other:b'); });
  const attachment = { file: new File(['draft'], 'draft.txt'), kind: 'file' as const, clientId: 'file', originalName: 'draft.txt', placeholder: '[FILE draft.txt]' };
  act(() => { hook.result.current[1]({ prompt: 'Unsent', attachments: [] }); hook.result.current[3]({ prompt: '', attachments: [attachment] }); });
  expect(getUnsentThreadDraftTitles()).toEqual(['Fix login', 'Review attachment']);
  hook.unmount(); registerThreadDraftTitle('device:a', 'Renamed login task');
  expect(getUnsentThreadDraftTitles()).toEqual(['Renamed login task', 'Review attachment']); expect(getUnsentThreadDraftCounts()).toEqual({ drafts: 2, attachments: 1 });
});

it('keeps draft text for a reload of this tab and names it for tabs and rows', () => {
  const { result } = renderHook(() => useThreadDrafts('dev:t'));
  const file = { file: new File(['x'], 'a.txt'), kind: 'file' as const, clientId: 'f', originalName: 'a.txt', placeholder: '[FILE a.txt]' };
  act(() => result.current[1]({ prompt: 'fix the [FILE a.txt] parser', attachments: [file] }));
  expect(JSON.parse(sessionStorage.getItem('pockymoe.threadDrafts.v1') ?? '{}')).toEqual({ 'dev:t': 'fix the [FILE a.txt] parser' });
  expect(getThreadDraftPreview('dev:t')).toBe('fix the parser');
  act(() => result.current[1]({ prompt: '', attachments: [] }));
  expect(sessionStorage.getItem('pockymoe.threadDrafts.v1')).toBeNull();
  expect(getThreadDraftPreview('dev:t')).toBeUndefined();
});

it('sends a seeded first prompt once', () => {
  seedThreadDraft('dev:new', 'hello', true);
  expect(takeThreadAutoSend('dev:new')).toBe(true);
  expect(takeThreadAutoSend('dev:new')).toBe(false);
  expect(getThreadDraftPreview('dev:new')).toBe('hello');
});
