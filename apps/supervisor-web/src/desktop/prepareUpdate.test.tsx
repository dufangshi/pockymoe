import { act, fireEvent, renderHook, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { prepareClientUpdate } from './prepareUpdate';
import { registerThreadDraftTitle, resetKeptDrafts, useThreadDrafts } from '../pages/useThreadDrafts';
import { revokeWorkspaceDocumentUnloadApproval } from '@pockymoe/thread-ui';
// The registry is browser-global so the lazy editor and host share protection.
// Seed the same memory store without pulling unbuilt shared source into Web TS.
const documentStores = (globalThis as unknown as Record<symbol, { stores: Map<string, Map<string, { key: string; content: string; baseContent: string; revision: number; phase: string }>> }>)[Symbol.for('remote-codex.workspace-documents')]!.stores;
beforeEach(() => { resetKeptDrafts(); documentStores.clear(); revokeWorkspaceDocumentUnloadApproval(); setLocale('en', false); });
afterEach(async () => { await act(async () => { await prepareClientUpdate(true); }); });
const unload = () => { const event = new Event('beforeunload', { cancelable: true }); window.dispatchEvent(event); return event.defaultPrevented; };
function pendingWork() {
  registerThreadDraftTitle('text', 'Fix login'); registerThreadDraftTitle('attachment', 'Review attachment');
  const hook = renderHook(() => useThreadDrafts('text', 'attachment'));
  const attachment = { file: new File(['draft'], 'draft.txt'), kind: 'file' as const, clientId: 'draft', originalName: 'draft.txt', placeholder: '[FILE draft.txt]' };
  act(() => { hook.result.current[1]({ prompt: 'Unsent', attachments: [] }); hook.result.current[3]({ prompt: '', attachments: [attachment] }); });
  documentStores.set('workspace', new Map([['draft.txt', { key: 'draft.txt', content: 'unsaved document', baseContent: 'base', revision: 1, phase: 'dirty' }]]));
  return hook;
}
it('confirms both unsent drafts and dirty files once, cancels without approving unload, and accepts explicitly', async () => {
  pendingWork(); let result!: ReturnType<typeof prepareClientUpdate>;
  act(() => { result = prepareClientUpdate(); });
  const dialog = await screen.findByRole('dialog', { name: 'Update and restart?' }); expect(dialog).toHaveTextContent('Unsent content will be lost: Fix login and Review attachment'); expect(dialog).toHaveTextContent('Drafts: 2. Attachments: 1. Unsaved workspace documents: 1.'); expect(screen.getByRole('button', { name: 'Update' })).toHaveClass('ui-action-primary');
  expect(prepareClientUpdate()).toBe(result);
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' })); expect(await result).toMatchObject({ ok: false }); expect(unload()).toBe(true);
  act(() => { result = prepareClientUpdate(); }); await screen.findByRole('dialog');
  fireEvent.click(screen.getByRole('button', { name: 'Update' })); expect(await result).toEqual({ ok: true, data: { prepared: true } }); expect(unload()).toBe(false); expect(unload()).toBe(false);
  await prepareClientUpdate(true); expect(unload()).toBe(true);
});
it('requires new consent if a draft or document changes while the confirmation is open', async () => {
  const hook = pendingWork(); let result!: ReturnType<typeof prepareClientUpdate>;
  act(() => { result = prepareClientUpdate(); }); await screen.findByRole('dialog');
  act(() => hook.result.current[1]({ prompt: 'Later typing', attachments: [] }));
  fireEvent.click(screen.getByRole('button', { name: 'Update' })); expect(await result).toMatchObject({ ok: false }); expect(unload()).toBe(true);
});
it('native cancellation dismisses an open confirmation and cannot approve a later close', async () => {
  pendingWork(); let result!: ReturnType<typeof prepareClientUpdate>;
  act(() => { result = prepareClientUpdate(); }); await screen.findByRole('dialog');
  await act(async () => { await prepareClientUpdate(true); }); expect(await result).toMatchObject({ ok: false }); expect(screen.queryByRole('dialog')).toBeNull(); expect(unload()).toBe(true);
});
it('requires no dialog for an empty window and still rejects edits made after consent', async () => {
  expect(await prepareClientUpdate()).toEqual({ ok: true, data: { prepared: true } }); expect(screen.queryByRole('dialog')).toBeNull();
  const hook = renderHook(() => useThreadDrafts('later')); act(() => hook.result.current[1]({ prompt: 'Appeared after preflight', attachments: [] }));
  expect(unload()).toBe(true);
});

it('still confirms dirty documents without inventing an unsent conversation title', async () => {
  documentStores.set('workspace', new Map([['file', { key: 'file', content: 'changed', baseContent: 'base', revision: 1, phase: 'dirty' }]]));
  let result!: ReturnType<typeof prepareClientUpdate>; act(() => { result = prepareClientUpdate(); });
  const dialog = await screen.findByRole('dialog', { name: 'Update and restart?' });
  expect(dialog).toHaveTextContent('Unsaved workspace documents: 1'); expect(dialog).not.toHaveTextContent('Unsent content');
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' })); expect(await result).toMatchObject({ ok: false });
});
it('an attachment-only composer prompts with its actual conversation title', async () => {
  registerThreadDraftTitle('attachment-only', 'Review screenshot');
  const hook = renderHook(() => useThreadDrafts('attachment-only'));
  const attachment = { file: new File(['draft'], 'draft.txt'), kind: 'file' as const, clientId: 'file', originalName: 'draft.txt', placeholder: '[FILE draft.txt]' };
  act(() => hook.result.current[1]({ prompt: '', attachments: [attachment] }));
  let result!: ReturnType<typeof prepareClientUpdate>; act(() => { result = prepareClientUpdate(); });
  expect(await screen.findByRole('dialog')).toHaveTextContent('Unsent content will be lost: Review screenshot');
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' })); expect(await result).toMatchObject({ ok: false });
});
