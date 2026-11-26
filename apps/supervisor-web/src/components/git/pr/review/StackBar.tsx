import { useState } from 'react';
import { GitMerge, Layers, RefreshCcw } from 'lucide-react';
import { Select, confirmAsync } from '@pockymoe/thread-ui';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { FormDialog } from '../../../FormDialog';
import type { PullDetail, Stack, StackLayer } from './reviewApi';
import { PullStateMark } from './parts';
import { rt, type ReviewMessage } from './reviewText';

const METHOD_LABEL: Record<string, ReviewMessage> = { squash: 'squash', merge: 'mergeCommit', rebase: 'rebase' };
const heads = (layers: StackLayer[]) => layers.filter(layer => layer.state === 'open').map(layer => ({ number: layer.number, headSha: layer.headSha }));

/** The PR's stack, bottom (on the base branch) to top: every layer one tap
    away, the open one marked, and the stack actions the host allows. */
export function StackBar({ stack, detail, busy, canRebase, onOpen, onAct }: {
  stack: Stack; detail: PullDetail; busy: boolean; canRebase: boolean;
  onOpen(layer: StackLayer): void;
  onAct(action: 'mergeStack' | 'rebaseStack', input: Record<string, unknown>): Promise<unknown>;
}) {
  useI18n();
  const [merging, setMerging] = useState(false);
  const index = stack.layers.findIndex(layer => layer.number === detail.number);
  const below = stack.layers.slice(0, index + 1).filter(layer => layer.state !== 'merged');
  const open = stack.layers.filter(layer => layer.state === 'open');
  const canMerge = detail.state === 'open' && (detail.capabilities.merge || stack.native) && below.every(layer => layer.state === 'open' && !layer.draft);
  return <nav className="pr-stack" aria-label={`${rt('stack')} · ${rt(stack.native ? 'stackNative' : 'stackInferred')}`}>
    <span className="pr-stack-label" title={rt(stack.native ? 'stackNative' : 'stackInferred')}><Layers aria-hidden="true" /><span>{rt('stackBase', { base: stack.base })}</span></span>
    <ol className="pr-stack-layers">
      {stack.layers.map((layer, position) => <li key={layer.number}>
        <button type="button" aria-current={layer.number === detail.number ? 'page' : undefined} disabled={layer.number === detail.number}
          title={`#${layer.number} ${layer.title}`} aria-label={`${rt('stackPosition', { i: position + 1, n: stack.layers.length })}: #${layer.number} ${layer.title}`} onClick={() => onOpen(layer)}>
          <PullStateMark state={layer.state} draft={layer.draft} /><span className="pr-stack-number">#{layer.number}</span><span className="pr-stack-title">{layer.title}</span>
        </button>
      </li>)}
    </ol>
    <span className="pr-stack-actions">
      {canMerge && <button type="button" className="pr-button" disabled={busy} onClick={() => setMerging(true)}><GitMerge aria-hidden="true" />{rt('mergeStack')}</button>}
      {canRebase && open.length > 0 && <button type="button" className="pr-button" disabled={busy} onClick={() => {
        void confirmAsync({ title: rt('rebaseStackTitle', { base: stack.base }), description: rt('rebaseStackBody', { n: open.length }), confirmLabel: rt('rebaseStack'), tone: 'danger' })
          .then(ok => { if (ok) void onAct('rebaseStack', { expectedHeads: heads(stack.layers) }); });
      }}><RefreshCcw aria-hidden="true" />{rt('rebaseStack')}</button>}
    </span>
    {merging && <MergeStackDialog stack={stack} layers={below} detail={detail} busy={busy} onClose={() => setMerging(false)}
      onMerge={async method => { const ok = await onAct('mergeStack', { method, expectedHeads: heads(below) }); if (ok) setMerging(false); }} />}
  </nav>;
}

function MergeStackDialog({ stack, layers, detail, busy, onClose, onMerge }: { stack: Stack; layers: StackLayer[]; detail: PullDetail; busy: boolean; onClose(): void; onMerge(method: string): Promise<void> }) {
  const methods = detail.mergeMethods.length ? detail.mergeMethods : ['merge'];
  const [method, setMethod] = useState(methods.includes('merge') && !stack.native ? 'merge' : methods[0]!);
  return <FormDialog secondary title={rt('mergeStackTitle', { n: layers.length })} description={rt(stack.native ? 'mergeStackNative' : 'mergeStackInferred', { base: stack.base })} onClose={onClose} busy={busy}>
    <form className="pr-review-form" onSubmit={event => { event.preventDefault(); void onMerge(method); }}>
      <ol className="pr-stack-merge-list">{layers.map(layer => <li key={layer.number}><PullStateMark state={layer.state} draft={layer.draft} /><span>#{layer.number}</span><span className="pr-stack-title">{layer.title}</span><code>{layer.headSha.slice(0, 7)}</code></li>)}</ol>
      {methods.length > 1 && <label>{rt('mergeMethod')}<Select value={method} onValueChange={setMethod} options={methods.map(item => ({ value: item, label: rt(METHOD_LABEL[item] ?? 'mergeCommit') }))} /></label>}
      <p className="pr-dim">{rt('mergeStackCredits')}</p>
      <div className="form-dialog-actions"><button type="button" data-dialog-cancel onClick={onClose}>{rt('cancel')}</button><button type="submit" className="ui-action-danger pr-dialog-danger" disabled={busy}>{rt('mergeLayers', { n: layers.length })}</button></div>
    </form>
  </FormDialog>;
}
