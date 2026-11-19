import { useCallback, useEffect, useRef, useState, type Dispatch, type MutableRefObject, type SetStateAction } from 'react';
import type { PromptAttachmentUpload } from '../lib/api';
interface Draft {
  prompt: string;
  attachments: PromptAttachmentUpload[];
}
const empty: Draft = { prompt: '', attachments: [] };
/** Drafts outlive the page that edits them: switching to a thread on another
    device remounts the workbench, and a half-written prompt must not vanish.
    The text also survives a reload of this tab (sessionStorage: this tab only,
    gone when it closes, never localStorage); attached files stay in memory, so a
    reload keeps the words and drops the files' placeholders. Leaving the page
    with a draft asks first (docs/ui-principles.md §4). */
const kept = new Map<string, Draft>();
const STORE_KEY = 'pockymoe.threadDrafts.v1';
/** Attachment and quote placeholders; their files do not survive a reload. */
const PLACEHOLDER = /\[(?:PHOTO|FILE|QUOTE) [^\]\n]*\]/g;
function restoreStoredDrafts() {
  try {
    const stored = JSON.parse(sessionStorage.getItem(STORE_KEY) ?? '{}') as Record<string, unknown>;
    for (const [key, prompt] of Object.entries(stored)) {
      if (typeof prompt !== 'string') continue;
      const text = prompt.replace(PLACEHOLDER, '').replace(/[ \t]+\n/g, '\n').trim();
      if (text && !kept.has(key)) kept.set(key, { prompt: text, attachments: [] });
    }
  } catch { /* No storage (private mode): drafts stay in memory only. */ }
}
function storeDrafts() {
  try {
    const texts = Object.fromEntries([...kept].filter(([, draft]) => draft.prompt.trim()).map(([key, draft]) => [key, draft.prompt]));
    if (Object.keys(texts).length) sessionStorage.setItem(STORE_KEY, JSON.stringify(texts));
    else sessionStorage.removeItem(STORE_KEY);
  } catch { /* Full or unavailable: the draft still holds in memory. */ }
}
if (typeof window !== 'undefined') restoreStoredDrafts();

/** A native client update asked for and got approval for these drafts. */
let unloadApproval: number | null = null;
export function approveThreadDraftUnload(revision: number | null) { unloadApproval = revision; }
if (typeof window !== 'undefined') {
  window.addEventListener('beforeunload', event => {
    if (!kept.size) return;
    if (unloadApproval !== null && unloadApproval === keptRevision) {
      if (event.isTrusted) unloadApproval = null;
      return;
    }
    event.preventDefault();
    event.returnValue = '';
  });
}

/** A thread created with its first prompt: the thread page sends it once open. */
const autoSend = new Set<string>();
export function seedThreadDraft(key: string, prompt: string, send: boolean) {
  if (!key || !prompt.trim()) return;
  kept.set(key, { prompt, attachments: [] });
  keptRevision++;
  storeDrafts();
  if (send) autoSend.add(key);
}
/** True once for a thread whose first prompt waits to be sent. */
export function takeThreadAutoSend(key: string) {
  return autoSend.delete(key);
}

/** The start of the unsent prompt kept for a thread, for its tab and row. */
export function getThreadDraftPreview(key: string): string | undefined {
  const draft = kept.get(key);
  if (!draft) return undefined;
  const text = draft.prompt.replace(PLACEHOLDER, ' ').replace(/\s+/g, ' ').trim();
  if (text) return text.length > 60 ? `${text.slice(0, 60)}…` : text;
  return draft.attachments[0]?.originalName;
}
const titles = new Map<string, string>();
export function registerThreadDraftTitle(key: string, title: string | undefined): void {
  if (key && title?.trim()) titles.set(key, title.trim());
}
/** Keep names with retained drafts, without persisting either names or content. */
export function useThreadDraftTitle(key: string, title: string | undefined): void {
  useEffect(() => { registerThreadDraftTitle(key, title); return () => { if (!kept.has(key)) titles.delete(key); }; }, [key, title]);
}
export function getUnsentThreadDraftTitles(): (string | undefined)[] {
  return [...kept.keys()].map(key => titles.get(key));
}
let keptRevision = 0;
export function getThreadDraftRevision(): number { return keptRevision; }
export function getUnsentThreadDraftCounts(): { drafts: number; attachments: number } {
  return { drafts: kept.size, attachments: [...kept.values()].reduce((count, draft) => count + draft.attachments.length, 0) };
}
function keep(key: string, draft: Draft | undefined) {
  if (!key) return;
  if (!draft || (!draft.prompt && !draft.attachments.length)) { if (kept.delete(key)) { keptRevision++; storeDrafts(); } }
  else if (kept.get(key) !== draft) { kept.set(key, draft); keptRevision++; storeDrafts(); }
}
/** For tests: forget every kept draft. */
export function resetKeptDrafts() { kept.clear(); titles.clear(); keptRevision++; storeDrafts(); }
function useDraftBinding(
  key: string,
  drafts: Record<string, Draft>,
  setDrafts: Dispatch<SetStateAction<Record<string, Draft>>>,
  owners: MutableRefObject<Record<string, object>>,
) {
  const active = useRef<{ key: string; owner: object } | null>(null);
  if (!active.current || active.current.key !== key) {
    active.current = { key, owner: {} };
    owners.current[key] = active.current.owner;
  }
  const owner = active.current.owner;
  const update = useCallback((action: SetStateAction<Draft>) => {
    if (owners.current[key] !== owner) return;
    setDrafts(current => {
      if (owners.current[key] !== owner) return current;
      const next = typeof action === 'function' ? action(current[key] ?? empty) : action;
      keep(key, next);
      return { ...current, [key]: next };
    });
  }, [key, owner, owners, setDrafts]);
  return [drafts[key] ?? empty, update] as const;
}
/** Both pane bindings share a workbench-owned store, including across primary swaps.
 * Draft bodies and File objects stay in memory, never in layout storage. */
export function useThreadDrafts(source: string, secondarySource = '') {
  const owners = useRef<Record<string, object>>({});
  const [drafts, setDrafts] = useState<Record<string, Draft>>(() => Object.fromEntries(kept));
  const primary = useDraftBinding(source, drafts, setDrafts, owners);
  const secondary = useDraftBinding(secondarySource, drafts, setDrafts, owners);
  return [...primary, ...secondary] as const;
}
