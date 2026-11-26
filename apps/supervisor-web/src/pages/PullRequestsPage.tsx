import { useCallback, useEffect, useRef, useState } from 'react';
import { useLocation, useSearchParams } from 'react-router-dom';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { ProductHeader } from '../components/ProductHeader';
import { WorkspacesFrame, useDesktopWorkspaceLayout } from '../desktop/WorkspacesFrame';
import { readSelectedRelayDeviceId, relayModeActive } from '../lib/api';
import { currentWorkspacesHref, relayDeviceIdFromPath } from '../lib/relayRoutes';
import { PullRequestList } from '../components/git/pr/review/PullRequestList';
import { PullRequestReview, type ReviewTab } from '../components/git/pr/review/PullRequestReview';
import { DEFAULT_QUERY, normalizeQuery } from '../components/git/pr/review/listModel';
import { fetchPullRequests, fetchViews, saveViews, type ListQuery, type PageResponse, type SavedView } from '../components/git/pr/review/reviewApi';
import { rt } from '../components/git/pr/review/reviewText';
import '../components/git/pr/review/review.css';

const TABS: ReviewTab[] = ['conversation', 'code', 'commits', 'checks', 'guide'];
const queryKey = (device: string | null) => `pockymoe.pullRequests.query:${device ?? 'local'}`;
function readQuery(device: string | null): ListQuery {
  try { return normalizeQuery(JSON.parse(localStorage.getItem(queryKey(device)) ?? 'null')); } catch { return DEFAULT_QUERY; }
}

/** Every pull request in this device's repositories, one place to review them.
    The list and the open PR sit side by side on wide screens; on phones the
    PR replaces the list and Back returns to it (`?pr=` keeps it in the URL). */
export function PullRequestsPage() {
  useI18n();
  const location = useLocation();
  const relayDeviceId = relayDeviceIdFromPath(location.pathname);
  const deviceId = relayDeviceId ?? (relayModeActive() ? readSelectedRelayDeviceId() : null);
  const desktopLayout = useDesktopWorkspaceLayout();
  const [params, setParams] = useSearchParams();
  const selected = params.get('pr');
  const tabParam = params.get('tab') as ReviewTab | null;
  const tab = tabParam && TABS.includes(tabParam) ? tabParam : undefined;
  const [query, setQueryState] = useState<ListQuery>(() => readQuery(deviceId));
  const [page, setPage] = useState<PageResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [views, setViews] = useState<SavedView[]>([]);
  const [revision, setRevision] = useState(0);
  const state = query.state;
  const loaded = useRef<string | null>(null);
  const setQuery = useCallback((next: ListQuery) => {
    setQueryState(next);
    try { localStorage.setItem(queryKey(deviceId), JSON.stringify(next)); } catch { /* Optional preference. */ }
  }, [deviceId]);
  useEffect(() => {
    const controller = new AbortController();
    setLoading(true);
    // A state change keeps the old rows visible until the new ones arrive.
    if (loaded.current !== `${deviceId}:${state}`) setError(null);
    void fetchPullRequests(deviceId, state, controller.signal).then(next => { setPage(next); setError(null); loaded.current = `${deviceId}:${state}`; })
      .catch(caught => { if (!controller.signal.aborted) setError(caught instanceof Error ? caught.message : String(caught)); })
      .finally(() => { if (!controller.signal.aborted) setLoading(false); });
    return () => controller.abort();
  }, [deviceId, state, revision]);
  useEffect(() => { void fetchViews(deviceId).then(value => setViews(value.views.map(view => ({ ...view, query: normalizeQuery(view.query) })))).catch(() => {}); }, [deviceId]);
  const updateViews = useCallback(async (next: SavedView[]) => { const saved = await saveViews(deviceId, next); setViews(saved.views.map(view => ({ ...view, query: normalizeQuery(view.query) }))); }, [deviceId]);
  const listSearch = (() => { const next = new URLSearchParams(params); next.delete('pr'); next.delete('tab'); return next.size ? `?${next}` : ''; })();
  const open = (url: string | null, nextTab?: ReviewTab) => {
    const next = new URLSearchParams(params);
    if (url) next.set('pr', url); else next.delete('pr');
    if (nextTab && nextTab !== 'conversation') next.set('tab', nextTab); else next.delete('tab');
    setParams(next, { replace: !!url && !!selected });
  };
  const content = <div className="pr-page" data-page="pull-requests">
    {/* Narrow, an open PR replaces the list: Back returns to the list first. */}
    {!desktopLayout && <ProductHeader title={rt('page')} {...(selected ? { backHref: `${location.pathname}${listSearch}`, backLabel: rt('back') } : { backHref: currentWorkspacesHref(), backLabel: rt('page') })} />}
    <div className="pr-split" data-detail={selected ? true : undefined}>
      <PullRequestList page={page} loading={loading} error={error} query={query} onQuery={setQuery} views={views} onViews={updateViews} selected={selected} onOpen={pull => open(pull.url)} onRefresh={() => setRevision(value => value + 1)} />
      {selected && <PullRequestReview key={selected} url={selected} deviceId={deviceId} tab={tab} onTab={next => open(selected, next)} onBack={() => open(null)} onChanged={() => setRevision(value => value + 1)} />}
    </div>
  </div>;
  return desktopLayout ? <WorkspacesFrame deviceId={deviceId} sharedOnly={false}>{content}</WorkspacesFrame> : content;
}

export default PullRequestsPage;
