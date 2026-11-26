import { describe, expect, it } from 'vitest';
import { activeFilters, DEFAULT_QUERY, filterPulls, matchingView, normalizeQuery, parseSearch, textScore } from './listModel';
import { pull } from './fixtures.test-data';

const viewers = { 'github.com': 'me' };
describe('pull request list model', () => {
  const pulls = [
    pull(1, { author: { login: 'me' }, checks: 'failed', reviewDecision: 'changesRequested' }),
    pull(2, { requestedReviewers: ['me'], reviewDecision: 'reviewRequired' }),
    pull(3, { draft: true, labels: [{ name: 'bug' }], title: 'Fix the bug in the lexer' }),
    pull(4, { reviewDecision: 'approved', checks: 'passed', headRef: 'feat/lexer-speed' }),
    pull(5, { state: 'merged' }),
  ];
  it('filters by involvement against the signed-in login of each host', () => {
    expect(filterPulls(pulls, { ...DEFAULT_QUERY, involvement: 'authored' }, viewers).map(p => p.number)).toEqual([1]);
    expect(filterPulls(pulls, { ...DEFAULT_QUERY, involvement: 'reviewing' }, viewers).map(p => p.number)).toEqual([2]);
    expect(filterPulls(pulls, { ...DEFAULT_QUERY, involvement: 'authored' }, {})).toEqual([]);
  });
  it('applies state, draft, review and checks filters and typed qualifiers', () => {
    expect(filterPulls(pulls, { ...DEFAULT_QUERY, state: 'merged' }, viewers).map(p => p.number)).toEqual([5]);
    expect(filterPulls(pulls, { ...DEFAULT_QUERY, draft: 'hide', checks: 'passed' }, viewers).map(p => p.number).sort()).toEqual([2, 4]);
    expect(filterPulls(pulls, { ...DEFAULT_QUERY, review: 'approved' }, viewers).map(p => p.number)).toEqual([4]);
    expect(filterPulls(pulls, { ...DEFAULT_QUERY, search: 'label:bug' }, viewers).map(p => p.number)).toEqual([3]);
    expect(filterPulls(pulls, { ...DEFAULT_QUERY, search: 'author:me' }, viewers).map(p => p.number)).toEqual([1]);
    expect(filterPulls(pulls, { ...DEFAULT_QUERY, search: 'is:draft' }, viewers).map(p => p.number)).toEqual([3]);
    expect(parseSearch('lexer -label:wip review:approved').qualifiers).toMatchObject({ excluded: ['wip'], review: 'approved' });
  });
  it('ranks free text before the sort and sorts by merge readiness', () => {
    expect(textScore(pull(4), ['4'])).toBe(100);
    expect(filterPulls(pulls, { ...DEFAULT_QUERY, search: 'lexer' }, viewers).map(p => p.number)).toEqual([3, 4]);
    // Ready: approved and passing first, failing next to last, drafts last.
    expect(filterPulls(pulls, DEFAULT_QUERY, viewers).map(p => p.number)).toEqual([4, 2, 1, 3]);
    expect(filterPulls(pulls, { ...DEFAULT_QUERY, sort: 'largest' }, viewers)[0]!.number).toBe(4);
  });
  it('normalizes stored queries and recognizes the view they match', () => {
    const query = normalizeQuery({ state: 'bogus', involvement: 'reviewing', sort: 'oldest', extra: 1 });
    expect(query).toEqual({ ...DEFAULT_QUERY, involvement: 'reviewing', sort: 'oldest' });
    expect(activeFilters(query)).toBe(1);
    expect(matchingView([{ id: 'v', name: 'Mine', query }], { ...query })?.id).toBe('v');
    expect(matchingView([{ id: 'v', name: 'Mine', query }], DEFAULT_QUERY)).toBeNull();
  });
});
