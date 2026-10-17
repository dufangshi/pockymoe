import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { Search, X } from 'lucide-react';
import {
  ApiError, fetchThreadConversationPage, fetchThreadTurnDetail, searchThreadMessages,
  type ConversationSearchMatch, type GlobalConversationSearchMatch, searchConversations,
} from '../lib/api';
import type { ThreadTurnDto } from '@pockymoe/shared';
import { ConversationSearchExcerpt, ConversationSearchScopePicker, type ConversationSearchScope } from '@pockymoe/thread-ui';
import { useSearchMessages } from './searchMessages';

type SearchMatch = ConversationSearchMatch | GlobalConversationSearchMatch;
type SearchResult = { matches: SearchMatch[]; hasMore: boolean; nextOffset?: number | null };
const isGlobal = (match: SearchMatch): match is GlobalConversationSearchMatch => 'threadId' in match;

function matchMessages(turns: ThreadTurnDto[], query: string): ConversationSearchMatch[] {
  return turns.flatMap(turn => turn.items
    .filter(item => (item.kind === 'userMessage' || item.kind === 'agentMessage')
      && item.text.toLowerCase().includes(query))
    .map(item => ({ turnId: turn.id, itemId: item.id,
      role: item.kind === 'userMessage' ? 'You' : 'Assistant', text: item.text })));
}
const matchKey = (match: SearchMatch) => `${isGlobal(match) ? match.threadId : ''}:${match.turnId}:${match.itemId}`;

export function ConversationSearch({ threadId, workspaceId, deviceLabel, allowGlobal, turns, open, onOpen, onClose, onSelect, onNavigate }: {
  threadId: string;
  workspaceId: string;
  deviceLabel: string;
  allowGlobal: boolean;
  onNavigate: (match: GlobalConversationSearchMatch, query: string) => void;
  turns: ThreadTurnDto[];
  open: boolean;
  onOpen: () => void;
  onClose: () => void;
  /** `query` is what was searched, for marking it where the reader lands. */
  onSelect: (turns: ThreadTurnDto[], turnId: string, itemId: string, query: string) => void;
}) {
  const labels = useSearchMessages();
  const root = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const restoreFocus = useRef(false);
  const listId = useId();
  const closeCallback = useRef(onClose);
  closeCallback.current = onClose;
  const [query, setQuery] = useState('');
  const [scope, setScope] = useState<ConversationSearchScope>('thread');
  const [offset, setOffset] = useState(0);
  // Fail closed if a live permission refresh removes global access.
  const effectiveScope = allowGlobal ? scope : 'thread';
  const normalized = query.trim().toLowerCase();
  const searchKey = `${effectiveScope}:${offset}:${normalized}`;
  const [remote, setRemote] = useState<{ query: string; result: SearchResult } | null>(null);
  const [loading, setLoading] = useState(false);
  const [selecting, setSelecting] = useState(false);
  const [error, setError] = useState('');
  const [scopeUnavailable, setScopeUnavailable] = useState(false);
  const [active, setActive] = useState(0);
  const legacy = useRef(false);
  const cache = useRef(new Map<string, { result: SearchResult; at: number }>());
  const selection = useRef<AbortController | null>(null);
  useEffect(() => () => selection.current?.abort(), []);
  useEffect(() => {
    if (!open) {
      selection.current?.abort(); setSelecting(false);
      if (restoreFocus.current) { trigger.current?.focus(); restoreFocus.current = false; }
      return;
    }
    input.current?.focus();
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !root.current?.contains(event.target)) closeCallback.current();
    };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [open]);

  useEffect(() => {
    setActive(0);
    setError('');
    setScopeUnavailable(false);
    if (!open || !normalized) { setLoading(false); return; }
    const cached = cache.current.get(searchKey);
    if (cached && Date.now() - cached.at < 30_000) {
      setRemote({ query: searchKey, result: cached.result });
      setLoading(false);
      return;
    }
    const controller = new AbortController();
    setLoading(true);
    const timer = window.setTimeout(async () => {
      try {
        let result: SearchResult | undefined;
        if (effectiveScope !== 'thread') {
          result = await searchConversations(normalized, effectiveScope === 'workspace' ? workspaceId : undefined, offset, controller.signal);
        }
        if (effectiveScope === 'thread' && !legacy.current) {
          try { result = await searchThreadMessages(threadId, normalized, controller.signal); }
          catch (e) {
            if (controller.signal.aborted) return;
            if (e instanceof ApiError && e.statusCode === 404) legacy.current = true;
            else throw e;
          }
        }
        if (!result) {
          const seen = new Set<string>();
          const matches: ConversationSearchMatch[] = [];
          let before: string | undefined;
          do {
            if (controller.signal.aborted) return;
            const page = await fetchThreadConversationPage(threadId, before, controller.signal);
            if (controller.signal.aborted) return;
            const fresh = page.turns.filter(turn => !seen.has(turn.id));
            if (!fresh.length) break;
            fresh.forEach(turn => seen.add(turn.id));
            matches.push(...matchMessages([...fresh].reverse(), normalized));
            setRemote({ query: searchKey, result: { matches: matches.slice(0, 50), hasMore: matches.length > 50 } });
            before = fresh[0]?.id;
            if (matches.length > 50 || seen.size >= (page.totalTurnCount ?? seen.size)) break;
          } while (before);
          result = { matches: matches.slice(0, 50), hasMore: matches.length > 50 };
        }
        if (controller.signal.aborted) return;
        // Bound retained searches; cache is scoped to this mounted thread only.
        if (cache.current.size >= 10) cache.current.delete(cache.current.keys().next().value!);
        cache.current.set(searchKey, { result, at: Date.now() });
        setRemote({ query: searchKey, result });
      } catch (e) {
        if (!controller.signal.aborted) {
          // A 404 can mean an older runtime or a removed workspace. Keep the
          // selected scope and never broaden it or scan histories on its behalf.
          if (effectiveScope !== 'thread' && e instanceof ApiError && e.statusCode === 404) {
            setScopeUnavailable(true);
          } else setError(e instanceof Error ? e.message : labels.failed);
        }
      } finally { if (!controller.signal.aborted) setLoading(false); }
    }, 250);
    return () => { window.clearTimeout(timer); controller.abort(); };
  }, [open, normalized, threadId, workspaceId, effectiveScope, offset, searchKey]);

  const matches = useMemo(() => {
    if (!normalized) return [];
    const merged = new Map<string, SearchMatch>();
    if (remote?.query === searchKey) remote.result.matches.forEach(match => merged.set(matchKey(match), match));
    // The current live conversation is immediately searchable, even offline.
    if (effectiveScope === 'thread') matchMessages([...turns].reverse(), normalized).forEach(match => merged.set(matchKey(match), match));
    return [...merged.values()].slice(0, 50);
  }, [normalized, remote, turns, searchKey, effectiveScope]);
  useEffect(() => {
    if (open && matches[active]) document.getElementById(`${listId}-${active}`)?.scrollIntoView?.({ block: 'nearest' });
  }, [open, active, matches, listId]);
  const close = () => { restoreFocus.current = true; onClose(); };
  const select = async (match: SearchMatch) => {
    if (isGlobal(match) && (match.threadId !== threadId || !match.turnId || !match.itemId)) {
      onNavigate(match, query.trim()); close(); return;
    }
    if (!match.turnId || !match.itemId) return;
    if (selecting) return;
    selection.current?.abort();
    const controller = new AbortController();
    selection.current = controller;
    setSelecting(true); setError('');
    try {
      const loaded = turns.find(turn => turn.id === match.turnId && !turn.hasDeferredItems);
      const turn = loaded ?? await fetchThreadTurnDetail(threadId, match.turnId);
      if (controller.signal.aborted) return;
      onSelect([turn], match.turnId, match.itemId, query.trim());
      close();
    } catch (e) {
      if (!controller.signal.aborted) setError(e instanceof Error ? e.message : labels.openFailed);
    } finally { if (!controller.signal.aborted) setSelecting(false); }
  };

  return <div ref={root} className={`workbench-search ${open ? 'is-open' : ''}`}>
    {!open ? <button ref={trigger} className="matter-search-trigger" aria-label={labels.trigger} onClick={onOpen}>
      <Search data-icon="search" /><span>{labels.trigger}</span>
    </button> : <>
      <div className="workbench-search-field">
        <ConversationSearchScopePicker value={effectiveScope} allowGlobal={allowGlobal} labels={labels}
          onChange={value => { selection.current?.abort(); setSelecting(false); setScope(value); setOffset(0); input.current?.focus(); }} />
        <Search data-icon="search" size={16} aria-hidden="true" />
        <input ref={input} role="combobox" aria-label={labels.input} placeholder={effectiveScope === 'thread' ? labels.placeholder : labels.globalPlaceholder}
          aria-autocomplete="list" aria-expanded={!!normalized} aria-controls={normalized ? listId : undefined}
          aria-activedescendant={normalized && matches[active] ? `${listId}-${active}` : undefined}
          value={query} maxLength={200} onChange={event => { setQuery(event.target.value); setOffset(0); }}
          onKeyDown={event => {
            if (event.key === 'Escape') { event.stopPropagation(); close(); }
            if ((event.key === 'ArrowDown' || event.key === 'ArrowUp') && matches.length) {
              event.preventDefault(); setActive(index => (index + (event.key === 'ArrowDown' ? 1 : matches.length - 1)) % matches.length);
            }
            if (event.key === 'Enter' && matches[active] && !event.nativeEvent.isComposing) {
              event.preventDefault(); void select(matches[active]);
            }
          }} />
        <button type="button" aria-label={labels.close} onClick={close}><X size={16} /></button>
      </div>
      {(normalized || error || scopeUnavailable) && <div className="workbench-search-dropdown" aria-label={labels.results}>
        <p role="status">{selecting ? labels.opening : loading ? labels.searching
          : labels.count(matches.length, remote?.query === searchKey && remote.result.hasMore)}</p>
        {effectiveScope !== 'thread' && <p>{labels.localScope} · {deviceLabel}</p>}
        {scopeUnavailable && <p role="alert">{labels.scopeUnavailable}</p>}
        {error && <p role="alert">{error}</p>}
        <div id={listId} className="workbench-search-results" role="listbox" aria-label={labels.matches}>
          {matches.map((match, index) => {
            return <button key={matchKey(match)} id={`${listId}-${index}`} type="button" role="option"
              aria-selected={index === active} disabled={selecting}
              onMouseEnter={() => setActive(index)} onMouseDown={event => event.preventDefault()}
              onClick={() => void select(match)}>
              <strong>{isGlobal(match) ? `${match.threadTitle} · ${match.workspaceLabel} · ${deviceLabel} · ` : ''}
                {match.role === 'You' ? labels.you : match.role === 'Title' ? labels.title : labels.assistant}</strong>
              <ConversationSearchExcerpt text={match.text} query={normalized} />
            </button>;
          })}
        </div>
        {!loading && !error && !scopeUnavailable && !matches.length && <p>{labels.empty}</p>}
        {effectiveScope === 'thread' && remote?.query === searchKey && remote.result.hasMore && <p>{labels.refine}</p>}
        {effectiveScope !== 'thread' && <div className="workbench-search-pagination">
          {offset > 0 && <button type="button" disabled={loading} onClick={() => setOffset(Math.max(0, offset - 50))}>{labels.previous}</button>}
          {remote?.query === searchKey && remote.result.hasMore && offset < 10_000 && <button type="button" disabled={loading}
            onClick={() => setOffset(remote.result.nextOffset ?? offset + 50)}>{labels.more}</button>}
        </div>}
      </div>}
    </>}
  </div>;
}
