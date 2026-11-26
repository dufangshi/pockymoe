import type { Check, ListQuery, PullSummary, SavedView } from './reviewApi';

export const DEFAULT_QUERY: ListQuery = { state: 'open', involvement: 'all', author: '', draft: 'all', review: 'all', checks: 'all', repository: '', search: '', sort: 'ready' };

export function repositoryKey(pull: Pick<PullSummary, 'repository'>) {
  return `${pull.repository.host}/${pull.repository.owner}/${pull.repository.name}`;
}

/** Typed qualifiers inside the search box (`author:me label:bug draft:false review:approved`). */
export function parseSearch(text: string) {
  const terms: string[] = [];
  const qualifiers: { author?: string; labels: string[]; excluded: string[]; draft?: boolean; review?: string } = { labels: [], excluded: [] };
  for (const word of text.trim().split(/\s+/).filter(Boolean)) {
    const match = /^(-?)(author|label|draft|review|is):(.+)$/i.exec(word);
    if (!match) { terms.push(word.toLowerCase()); continue; }
    const [, negate, key, value] = match;
    const name = key!.toLowerCase();
    if (name === 'author') qualifiers.author = value!.replace(/^@/, '');
    else if (name === 'label') (negate ? qualifiers.excluded : qualifiers.labels).push(...value!.split(',').map(label => label.toLowerCase()));
    else if (name === 'draft') qualifiers.draft = value!.toLowerCase() === 'true';
    else if (name === 'is' && value!.toLowerCase() === 'draft') qualifiers.draft = !negate;
    else if (name === 'review') qualifiers.review = value!.toLowerCase();
  }
  return { terms, qualifiers };
}

const viewerOf = (pull: PullSummary, viewers: Record<string, string | null>) => viewers[pull.repository.host] ?? null;

function matchesAuthor(pull: PullSummary, author: string, viewers: Record<string, string | null>) {
  const login = author === 'me' || author === '@me' ? viewerOf(pull, viewers) : author;
  return !!login && pull.author?.login.toLowerCase() === login.toLowerCase();
}

/** How well a PR matches the free text: higher first, 0 drops it. */
export function textScore(pull: PullSummary, terms: string[]) {
  if (!terms.length) return 1;
  const title = pull.title.toLowerCase();
  const whole = terms.join(' ');
  if (terms.length === 1 && (terms[0] === String(pull.number) || terms[0] === `#${pull.number}`)) return 100;
  if (title === whole) return 90;
  if (title.includes(whole)) return 80;
  const haystack = [title, pull.headRef, pull.baseRef, pull.author?.login ?? '', `${pull.repository.owner}/${pull.repository.name}`, ...pull.labels.map(label => label.name)].join(' ').toLowerCase();
  if (terms.every(term => haystack.includes(term))) {
    if (terms.every(term => title.includes(term))) return 70;
    if (terms.some(term => pull.headRef.toLowerCase().includes(term))) return 60;
    if (terms.some(term => (pull.author?.login ?? '').toLowerCase().includes(term))) return 50;
    return 40;
  }
  return 0;
}

/** Tiers for "merge readiness": ready to merge first, conflicting and closed last. */
function readiness(pull: PullSummary) {
  if (pull.state !== 'open') return 4;
  if (pull.draft) return 3;
  if (pull.reviewDecision === 'approved' && pull.checks === 'passed') return 0;
  if (pull.checks === 'passed' || pull.checks === 'none') return 1;
  return pull.checks === 'failed' || pull.reviewDecision === 'changesRequested' ? 2.5 : 2;
}
const size = (pull: PullSummary) => (pull.additions ?? 0) + (pull.deletions ?? 0);

export function filterPulls(pulls: PullSummary[], query: ListQuery, viewers: Record<string, string | null>) {
  const { terms, qualifiers } = parseSearch(query.search);
  const author = qualifiers.author ?? query.author;
  const review = qualifiers.review ?? query.review;
  const draft = qualifiers.draft === undefined ? query.draft : qualifiers.draft ? 'only' : 'hide';
  const scored: { pull: PullSummary; score: number }[] = [];
  for (const pull of pulls) {
    if (query.state !== 'all' && pull.state !== query.state) continue;
    if (query.repository && repositoryKey(pull) !== query.repository) continue;
    if (query.involvement === 'authored' && !matchesAuthor(pull, 'me', viewers)) continue;
    if (query.involvement === 'reviewing') {
      const viewer = viewerOf(pull, viewers);
      if (!viewer || !pull.requestedReviewers.some(login => login.toLowerCase() === viewer.toLowerCase())) continue;
    }
    if (author && !matchesAuthor(pull, author, viewers)) continue;
    if (draft === 'only' && !pull.draft) continue;
    if (draft === 'hide' && pull.draft) continue;
    if (review !== 'all') {
      const decision = pull.reviewDecision;
      if (review === 'none' ? decision !== null : review === 'changesrequested' || review === 'changesRequested' ? decision !== 'changesRequested' : review === 'reviewrequired' || review === 'reviewRequired' ? decision !== 'reviewRequired' : decision !== review) continue;
    }
    if (query.checks !== 'all' && pull.checks !== query.checks) continue;
    const labels = pull.labels.map(label => label.name.toLowerCase());
    if (qualifiers.labels.length && !qualifiers.labels.some(label => labels.includes(label))) continue;
    if (qualifiers.excluded.some(label => labels.includes(label))) continue;
    const score = textScore(pull, terms);
    if (!score) continue;
    scored.push({ pull, score });
  }
  const order = (a: PullSummary, b: PullSummary) => {
    switch (query.sort) {
      case 'newest': return b.createdAt.localeCompare(a.createdAt);
      case 'oldest': return a.createdAt.localeCompare(b.createdAt);
      case 'largest': return size(b) - size(a);
      case 'smallest': return size(a) - size(b);
      case 'ready': return readiness(a) - readiness(b) || b.updatedAt.localeCompare(a.updatedAt);
      default: return b.updatedAt.localeCompare(a.updatedAt);
    }
  };
  // Free text ranks first; the chosen sort orders equal matches.
  return scored.sort((a, b) => (terms.length ? b.score - a.score : 0) || order(a.pull, b.pull)).map(item => item.pull);
}

/** How many filters differ from the default (search and sort are not filters). */
export function activeFilters(query: ListQuery) {
  return (['involvement', 'author', 'draft', 'review', 'checks', 'repository'] as const).filter(key => query[key] !== DEFAULT_QUERY[key]).length;
}

export function sameQuery(a: ListQuery, b: ListQuery) {
  return (Object.keys(DEFAULT_QUERY) as (keyof ListQuery)[]).every(key => a[key] === b[key]);
}

export function normalizeQuery(value: unknown): ListQuery {
  const input = (value && typeof value === 'object' ? value : {}) as Partial<Record<keyof ListQuery, unknown>>;
  const pick = <K extends keyof ListQuery>(key: K, allowed: readonly ListQuery[K][]) => (allowed as readonly unknown[]).includes(input[key]) ? input[key] as ListQuery[K] : DEFAULT_QUERY[key];
  return {
    state: pick('state', ['open', 'closed', 'merged', 'all']),
    involvement: pick('involvement', ['all', 'authored', 'reviewing']),
    author: typeof input.author === 'string' ? input.author.slice(0, 100) : '',
    draft: pick('draft', ['all', 'only', 'hide']),
    review: pick('review', ['all', 'approved', 'changesRequested', 'reviewRequired', 'none']),
    checks: pick('checks', ['all', 'passed', 'failed', 'pending']),
    repository: typeof input.repository === 'string' ? input.repository.slice(0, 300) : '',
    search: typeof input.search === 'string' ? input.search.slice(0, 200) : '',
    sort: pick('sort', ['ready', 'updated', 'newest', 'oldest', 'largest', 'smallest']),
  };
}

export function matchingView(views: SavedView[], query: ListQuery) {
  return views.find(view => sameQuery(normalizeQuery(view.query), query)) ?? null;
}

export function checkCounts(checks: Check[]) {
  return { passed: checks.filter(check => check.state === 'passed').length, failed: checks.filter(check => check.state === 'failed' || check.state === 'cancelled').length, pending: checks.filter(check => check.state === 'pending').length };
}
