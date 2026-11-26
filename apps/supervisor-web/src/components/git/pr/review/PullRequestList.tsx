import { useMemo, useState } from 'react';
import { Bookmark, RefreshCw, Search, X } from 'lucide-react';
import { ErrorMark, Select, confirmAsync } from '@pockymoe/thread-ui';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { FormDialog } from '../../../FormDialog';
import { activeFilters, DEFAULT_QUERY, filterPulls, matchingView } from './listModel';
import type { ListQuery, PageResponse, PullSummary, SavedView } from './reviewApi';
import { Ago, ChecksMark, DecisionMark, MenuButton, PullStateMark, StackChip, Stat } from './parts';
import { rt } from './reviewText';

export interface PullRequestListProps {
  page: PageResponse | null;
  loading: boolean;
  error: string | null;
  query: ListQuery;
  onQuery(next: ListQuery): void;
  views: SavedView[];
  onViews(next: SavedView[]): Promise<void>;
  selected: string | null;
  onOpen(pull: PullSummary): void;
  onRefresh(): void;
}

/** The device's PRs: one search box, one row of filter pills, then the rows.
    State is read from the hosts; every other filter narrows the same list. */
export function PullRequestList({ page, loading, error, query, onQuery, views, onViews, selected, onOpen, onRefresh }: PullRequestListProps) {
  useI18n();
  const [naming, setNaming] = useState<{ id: string | null; name: string } | null>(null);
  const pulls = useMemo(() => page ? filterPulls(page.pulls, query, page.viewers) : [], [page, query]);
  const repositories = useMemo(() => (page?.repositories ?? []).map(entry => ({ value: `${entry.repository.host}/${entry.repository.owner}/${entry.repository.name}`, label: `${entry.repository.owner}/${entry.repository.name}` })), [page]);
  const failed = (page?.repositories ?? []).filter(entry => entry.error);
  const set = <K extends keyof ListQuery>(key: K, value: ListQuery[K]) => onQuery({ ...query, [key]: value });
  const current = matchingView(views, query);
  const filters = activeFilters(query);
  const saveNamed = async () => {
    if (!naming?.name.trim()) return;
    const name = naming.name.trim().slice(0, 80);
    const next = naming.id ? views.map(view => view.id === naming.id ? { ...view, name } : view) : [...views, { id: crypto.randomUUID(), name, query }];
    await onViews(next);
    setNaming(null);
  };
  return <section className="pr-list" aria-label={rt('page')}>
    <div className="pr-list-bar">
      <label className="pr-search"><Search aria-hidden="true" /><input type="search" aria-label={rt('searchLabel')} placeholder={rt('search')} value={query.search} onChange={event => set('search', event.target.value)} spellCheck={false} /></label>
      <MenuButton label={rt('views')} icon={<Bookmark aria-hidden="true" />} className={current ? 'is-active' : ''} items={[
        { label: rt('defaultView'), checked: false, onSelect: () => onQuery(DEFAULT_QUERY) },
        ...views.map(view => ({ label: view.name, checked: current?.id === view.id, onSelect: () => onQuery({ ...DEFAULT_QUERY, ...view.query }) })),
        { label: '', separator: true },
        { label: rt('saveView'), onSelect: () => setNaming({ id: null, name: query.involvement === 'reviewing' ? rt('reviewing') : '' }) },
        ...(current ? [
          { label: rt('renameView'), onSelect: () => setNaming({ id: current.id, name: current.name }) },
          { label: rt('deleteView'), danger: true, onSelect: () => { void confirmAsync({ title: rt('deleteView'), description: current.name, confirmLabel: rt('deleteView'), tone: 'danger' }).then(ok => { if (ok) void onViews(views.filter(view => view.id !== current.id)); }); } },
        ] : []),
      ]} />
      <button type="button" className="pr-icon" onClick={onRefresh} disabled={loading} aria-label={rt('refresh')} title={rt('refresh')}><RefreshCw aria-hidden="true" className={loading ? 'request-spin' : undefined} /></button>
    </div>
    <div className="pr-pills" role="group" aria-label={rt('filters')}>
      <Select aria-label={rt('page')} value={query.state} onValueChange={value => set('state', value as ListQuery['state'])} options={[{ value: 'open', label: rt('open') }, { value: 'merged', label: rt('merged') }, { value: 'closed', label: rt('closed') }, { value: 'all', label: rt('all') }]} />
      <Select aria-label={rt('involvement')} value={query.involvement} onValueChange={value => set('involvement', value as ListQuery['involvement'])} options={[{ value: 'all', label: rt('anyone') }, { value: 'reviewing', label: rt('reviewing') }, { value: 'authored', label: rt('authored') }]} />
      <Select aria-label={rt('review')} value={query.review} onValueChange={value => set('review', value as ListQuery['review'])} options={[{ value: 'all', label: rt('reviewAll') }, { value: 'approved', label: rt('approved') }, { value: 'changesRequested', label: rt('changesRequested') }, { value: 'reviewRequired', label: rt('reviewRequired') }, { value: 'none', label: rt('noReview') }]} />
      <Select aria-label={rt('checks')} value={query.checks} onValueChange={value => set('checks', value as ListQuery['checks'])} options={[{ value: 'all', label: rt('checksAll') }, { value: 'passed', label: rt('passing') }, { value: 'failed', label: rt('failing') }, { value: 'pending', label: rt('pending') }]} />
      <Select aria-label={rt('draft')} value={query.draft} onValueChange={value => set('draft', value as ListQuery['draft'])} options={[{ value: 'all', label: rt('draftAll') }, { value: 'only', label: rt('draftOnly') }, { value: 'hide', label: rt('draftHide') }]} />
      {repositories.length > 1 && <Select aria-label={rt('repository')} value={query.repository} onValueChange={value => set('repository', value)} options={[{ value: '', label: rt('repositoryAll') }, ...repositories]} />}
      <Select aria-label={rt('sort')} value={query.sort} onValueChange={value => set('sort', value as ListQuery['sort'])} options={[{ value: 'ready', label: rt('sortReady') }, { value: 'updated', label: rt('sortUpdated') }, { value: 'newest', label: rt('sortNewest') }, { value: 'oldest', label: rt('sortOldest') }, { value: 'largest', label: rt('sortLargest') }, { value: 'smallest', label: rt('sortSmallest') }]} />
      {filters > 0 && <button type="button" className="pr-clear" onClick={() => onQuery({ ...DEFAULT_QUERY, state: query.state, search: query.search, sort: query.sort })}><X aria-hidden="true" />{rt('clearFilters')}</button>}
    </div>
    {(error || failed.length > 0) && <div className="pr-list-note">
      {error ? <ErrorMark message={error} onRetry={onRefresh} /> : <><span>{rt('repoFailed', { n: failed.length })}</span><ErrorMark message={failed.map(entry => `${entry.repository.owner}/${entry.repository.name}: ${entry.error}`).join('\n')} onRetry={onRefresh} /></>}
    </div>}
    {page && !page.repositories.length && <p className="pr-empty">{rt('noRepositories')}</p>}
    {page && page.repositories.length > 0 && !pulls.length && !loading && <p className="pr-empty">{rt('empty')}</p>}
    {!page && loading && <p className="pr-empty" aria-busy="true">{rt('loading')}</p>}
    <ul className="pr-rows">
      {pulls.map(pull => <li key={pull.url}>
        <button type="button" className="pr-row" aria-current={selected === pull.url || undefined} onClick={() => onOpen(pull)}>
          <PullStateMark state={pull.state} draft={pull.draft} />
          <span className="pr-row-main">
            <span className="pr-row-title"><span className="pr-row-title-text">{pull.title}</span><ChecksMark checks={pull.checks} /><DecisionMark decision={pull.reviewDecision} /></span>
            <span className="pr-row-meta">
              <span>{repositories.length > 1 ? `${pull.repository.name} ` : ''}#{pull.number}</span>
              <StackChip stack={pull.stack} />
              {pull.author && <span>{pull.author.login}</span>}
              <Ago value={pull.updatedAt} />
              <Stat additions={pull.additions} deletions={pull.deletions} />
              {pull.labels.slice(0, 3).map(label => <span key={label.name} className="pr-label" style={label.color ? { ['--pr-label' as string]: `#${label.color.replace(/^#/, '')}` } : undefined}>{label.name}</span>)}
            </span>
          </span>
        </button>
      </li>)}
    </ul>
    {naming && <FormDialog secondary title={naming.id ? rt('renameView') : rt('saveView')} description={rt('viewHint')} onClose={() => setNaming(null)}>
      <form className="pr-review-form" onSubmit={event => { event.preventDefault(); void saveNamed(); }}>
        <label>{rt('viewName')}<input autoFocus maxLength={80} required value={naming.name} onChange={event => setNaming({ ...naming, name: event.target.value })} /></label>
        <div className="form-dialog-actions"><button type="button" data-dialog-cancel onClick={() => setNaming(null)}>{rt('cancel')}</button><button type="submit" className="relay-button-primary" disabled={!naming.name.trim()}>{rt('save')}</button></div>
      </form>
    </FormDialog>}
  </section>;
}

