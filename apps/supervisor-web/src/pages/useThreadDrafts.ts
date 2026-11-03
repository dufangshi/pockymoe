import { useCallback, useEffect, useRef, useState, type Dispatch, type MutableRefObject, type SetStateAction } from 'react';
import type { PromptAttachmentUpload } from '../lib/api';
interface Draft {
  prompt: string;
  attachments: PromptAttachmentUpload[];
}
const empty: Draft = { prompt: '', attachments: [] };
/** Drafts outlive the page that edits them: switching to a thread on another
    device remounts the workbench, and a half-written prompt must not vanish.
    Memory only (this tab), never storage. */
const kept = new Map<string, Draft>();
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
  if (!draft || (!draft.prompt && !draft.attachments.length)) { if (kept.delete(key)) keptRevision++; }
  else if (kept.get(key) !== draft) { kept.set(key, draft); keptRevision++; }
}
/** For tests: forget every kept draft. */
export function resetKeptDrafts() { kept.clear(); titles.clear(); keptRevision++; }
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
