import { useEffect, useMemo, useRef, useState } from 'react';
import { PathChip, type PromptQuote } from '@pockymoe/thread-ui';
import { FormDialog } from '../FormDialog';
import { translate } from '@pockymoe/thread-ui/i18n';
import { gitText as gt } from '../../lib/gitI18n';
import { parsePatch, type ParsedPatch, type PatchLine } from './patch';
import type { Write } from './GitActions';

/** Keep Git's exact file header and one complete hunk, including newline markers. */
export function hunkPatches(patch: string): Map<string, string[]> {
  const result = new Map<string, string[]>();
  for (const file of patch.split(/(?=^diff --git )/m)) {
    const chunks = file.split(/(?=^@@ )/m);
    const header = chunks.shift() ?? '';
    const path = parsePatch(file).files[0]?.path;
    if (path && !header.includes('rename from')) result.set(path, chunks.filter(hunk => hunk.startsWith('@@ ')).map(hunk => header + hunk));
  }
  return result;
}
function numbered(lines: PatchLine[]) {
  let old = 0, next = 0, row = 1, deleted = 0, added = 0;
  return lines.map(line => {
    const match = /^@@ -(\d+)(?:,\d+)? \+(\d+)/.exec(line.text);
    if (match) { old = Number(match[1]); next = Number(match[2]); }
    const change = line.kind === 'del' || line.kind === 'add';
    if (!change) { row += Math.max(deleted, added); deleted = 0; added = 0; }
    const gridRow = change ? row + (line.kind === 'del' ? deleted++ : added++) : row++;
    const number = line.kind === 'del' ? old : next;
    if (line.kind === 'del' || line.kind === 'ctx') old++;
    if (line.kind === 'add' || line.kind === 'ctx') next++;
    return { ...line, number, gridRow };
  });
}
export function ReviewPatch({ patch, parsed, split, collapsed, area, write, busy, onReviewQuote, single, onSelect }: { single: boolean; onSelect(path: string): void; patch: string; parsed: ParsedPatch; split: boolean; collapsed: boolean; area: string; write: Write | undefined; busy: boolean; onReviewQuote?: ((quote: PromptQuote) => boolean) | undefined }) {
  const root = useRef<HTMLDivElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const [finding, setFinding] = useState(false);
  const [query, setQuery] = useState('');
  const [match, setMatch] = useState(0);
  const [selection, setSelection] = useState<PromptQuote | null>(null);
  const [commenting, setCommenting] = useState(false);
  const [comment, setComment] = useState('');
  const hunks = useMemo(() => hunkPatches(patch), [patch]);
  const [overrides, setOverrides] = useState<Record<string, boolean>>({});
  useEffect(() => { setOverrides({}); }, [collapsed, patch]);
  const move = (delta: number) => { const matches = root.current?.querySelectorAll<HTMLElement>('[data-match="true"]'); if (!matches?.length) return; const index = (match + delta + matches.length) % matches.length; setMatch(index); matches[index]?.scrollIntoView?.({ block: 'center' }); };
  const capture = () => {
    // Dialog pointer events bubble through React portals. Keep the selected
    // lines while editing/submitting the comment, even after DOM selection clears.
    if (!onReviewQuote || commenting) return;
    const selected = window.getSelection();
    if (!selected || selected.isCollapsed || !root.current?.contains(selected.anchorNode) || !root.current.contains(selected.focusNode)) { setSelection(null); return; }
    const parent = (node: Node | null) => (node instanceof Element ? node : node?.parentElement)?.closest<HTMLElement>('[data-line]');
    const a = parent(selected.anchorNode), b = parent(selected.focusNode);
    if (!a || !b || a.dataset.file !== b.dataset.file || a.dataset.side !== b.dataset.side) return;
    const start = Math.min(Number(a.dataset.line), Number(b.dataset.line)), end = Math.max(Number(a.dataset.line), Number(b.dataset.line));
    const text = Array.from(root.current.querySelectorAll<HTMLElement>('[data-line]')).filter(node => node.dataset.file === a.dataset.file && node.dataset.side === a.dataset.side && Number(node.dataset.line) >= start && Number(node.dataset.line) <= end).map(node => node.dataset.text).join('\n');
    setSelection({ path: a.dataset.file!, startLine: start, endLine: end, text, from: 'user', turn: null, turnId: null, itemId: null });
  };
  return <div ref={root} className="thread-git-review thread-git-patch" data-split={split || undefined} onMouseUp={capture} onTouchEnd={capture} onKeyDown={event => {
    if ((event.metaKey || event.ctrlKey) && event.key === 'f') { event.preventDefault(); setFinding(true); queueMicrotask(() => search.current?.focus()); }
  }} tabIndex={0}>
    <div className="thread-git-actions"><button onClick={() => { setFinding(value => !value); queueMicrotask(() => search.current?.focus()); }}>{gt('find')}</button>{selection && <button onClick={() => setCommenting(true)}>{gt('comment')}</button>}</div>
    {finding && <div className="thread-git-actions"><input ref={search} aria-label={gt('find')} value={query} onChange={event => { setQuery(event.target.value); setMatch(0); }} onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); move(event.shiftKey ? -1 : 1); } if (event.key === 'Escape') { event.stopPropagation(); setFinding(false); setQuery(''); } }} /><button onClick={() => move(-1)} aria-label={gt('previousMatch')}>↑</button><button onClick={() => move(1)} aria-label={gt('nextMatch')}>↓</button><button onClick={() => { setFinding(false); setQuery(''); }}>{gt('closeFind')}</button></div>}
    {parsed.preamble.length > 0 && <pre>{parsed.preamble.map(line => line.text).join('\n')}</pre>}
    {parsed.files.map(file => {
      const matches = !!query && file.lines.some(line => line.text.toLocaleLowerCase().includes(query.toLocaleLowerCase()));
      const open = matches || (overrides[file.path] ?? !collapsed);
      let hunk = -1;
      return <section key={file.path} className="thread-git-patch-file"><div className="thread-git-actions"><button aria-expanded={open} onClick={() => setOverrides(value => ({ ...value, [file.path]: !open }))}>{open ? '⌄' : '›'}</button>{!single ? <button aria-label={translate('workbench.gitViewFile', { path: file.path })} onClick={() => onSelect(file.path)}>{file.path}{file.binary ? ` · ${gt('binary')}` : ''}</button> : <PathChip path={file.path} />}</div>
        {file.binary && <p className="thread-git-file-note">{gt(file.created ? 'binaryNew' : file.removed ? 'binaryDeleted' : 'binaryChanged')}</p>}
        {file.oldPath && <p className="thread-git-file-note">{gt(file.lines.some(line => line.kind === 'hunk') ? 'renamedFrom' : 'renamedOnly', { path: file.oldPath })}</p>}
        {!file.binary && file.created && !file.lines.some(line => line.kind === 'hunk') && <p>{gt('emptyFile')}</p>}
        {open && <pre>{numbered(file.lines).map((line, index) => {
          if (line.kind === 'meta') return null;
          if (line.kind === 'hunk') { hunk++; const patch = hunks.get(file.path)?.[hunk]; return <span key={index} className="thread-git-line" style={{ gridRow: split ? line.gridRow : undefined }} data-kind="hunk">{line.text}{write && patch && (area === 'worktree' || area === 'index') && <span className="thread-git-actions">{(area === 'index' ? ['unstage'] as const : ['stage', 'discard'] as const).map(action => <button key={action} disabled={busy} onClick={() => { void write(`git/${action}`, { patch }).catch(() => {}); }}>{gt(action)} {gt('hunk')}</button>)}</span>}{'\n'}</span>; }
          const hit = !!query && line.text.toLocaleLowerCase().includes(query.toLocaleLowerCase());
          return <span key={index} className="thread-git-line" style={{ gridRow: split ? line.gridRow : undefined }} data-kind={line.kind} data-match={hit || undefined} data-file={file.path} data-line={['ctx', 'add', 'del'].includes(line.kind) ? line.number : undefined} data-side={line.kind === 'del' ? 'old' : 'new'} data-text={line.text.slice(1)}>{line.text || ' '}{'\n'}</span>;
        })}</pre>}
      </section>;
    })}
    {commenting && selection && <FormDialog secondary title={gt('addComment')} onClose={() => setCommenting(false)}><PathChip path={`${selection.path}:${selection.startLine}–${selection.endLine}`} /><blockquote>{selection.text}</blockquote><textarea aria-label={gt('comment')} value={comment} onChange={event => setComment(event.target.value)} /><div className="form-dialog-actions"><button onClick={() => setCommenting(false)}>{gt('cancel')}</button><button className="relay-button-primary" disabled={!comment.trim()} onClick={() => { if (onReviewQuote?.({ ...selection, comment: comment.trim() })) { setComment(''); setSelection(null); setCommenting(false); } }}>{gt('addComment')}</button></div></FormDialog>}
  </div>;
}
