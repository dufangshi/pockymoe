import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { detail, files } from './fixtures.test-data';

const calls: { action: string; input: Record<string, unknown> }[] = [];
let head = 'head-7';
let current = detail();
vi.mock('./reviewApi', async original => ({
  ...await original<typeof import('./reviewApi')>(),
  reviewAction: vi.fn(async (_device: unknown, action: string, _url: string, input: Record<string, unknown> = {}) => {
    calls.push({ action, input });
    if (action === 'detail') return current;
    if (action === 'files') return files;
    if (action === 'head') return { headSha: head };
    if (action === 'drafts' || action === 'draftSave' || action === 'draftDelete') return { drafts: current.drafts };
    if (action === 'mergeMessage') return { title: 'Stream tokens (#7)', body: '* One' };
    return { ok: true };
  }),
}));
const { PullRequestReview } = await import('./PullRequestReview');

beforeEach(() => { setLocale('en'); calls.length = 0; head = 'head-7'; current = detail(); });
const renderReview = (tab?: 'code') => render(<MemoryRouter><PullRequestReview url="https://github.com/fixture/parser/pull/7" tab={tab} /></MemoryRouter>);

it('submits the drafts as a review of the head that was loaded', async () => {
  current = detail({ body: '<!-- template note -->\nBody text' });
  renderReview();
  expect(await screen.findByText('Body text')).toBeTruthy();
  expect(screen.queryByText(/template note/)).toBeNull();
  await screen.findByRole('heading', { name: /Stream tokens/ });
  fireEvent.click(screen.getByRole('button', { name: /^Review/ }));
  const dialog = await screen.findByRole('dialog');
  expect(within(dialog).getByText('1 pending comment')).toBeTruthy();
  fireEvent.click(within(dialog).getByRole('radio', { name: 'Request changes' }));
  fireEvent.change(within(dialog).getByLabelText(/Summary/), { target: { value: 'Two notes' } });
  fireEvent.click(within(dialog).getByRole('button', { name: 'Submit review' }));
  await waitFor(() => expect(calls.find(call => call.action === 'submit')?.input).toEqual({ event: 'requestChanges', body: 'Two notes', expectedHead: 'head-7' }));
});

it('notices new commits and moves the reviewed head only after Load new commits', async () => {
  renderReview();
  await screen.findByRole('heading', { name: /Stream tokens/ });
  head = 'head-8';
  await act(async () => { document.dispatchEvent(new Event('visibilitychange')); });
  expect(await screen.findByText(/New commits were pushed/)).toBeTruthy();
  current = detail({ headSha: 'head-8' });
  fireEvent.click(screen.getByRole('button', { name: 'Load new commits' }));
  await waitFor(() => expect(screen.queryByText(/New commits were pushed/)).toBeNull());
  fireEvent.click(screen.getByRole('button', { name: /^Merge$/ }));
  const dialog = await screen.findByRole('dialog');
  // Remove agent credits starts ticked (device setting) and previews the cleaned message.
  expect(await within(dialog).findByText('Stream tokens (#7)')).toBeTruthy();
  fireEvent.click(within(dialog).getByRole('button', { name: 'Merge' }));
  await waitFor(() => expect(calls.find(call => call.action === 'merge')?.input).toMatchObject({ method: 'squash', removeAgentCredits: true, expectedHead: 'head-8' }));
});

it('marks files viewed with their revision and shows threads and drafts at their lines', async () => {
  renderReview('code');
  expect(await screen.findByText('Why u8?')).toBeTruthy();
  expect(screen.getByText('Name it')).toBeTruthy();
  expect(screen.getAllByText('Changed').length).toBeGreaterThan(0);
  fireEvent.click(screen.getAllByRole('button', { name: 'Discard' })[0]!);
  await waitFor(() => expect(calls.some(call => call.action === 'draftDelete' && call.input.id === 'd1')).toBe(true));
  fireEvent.click(screen.getByRole('checkbox', { name: 'Viewed: src/a.rs' }));
  await waitFor(() => expect(calls.find(call => call.action === 'viewed')?.input).toEqual({ paths: ['src/a.rs'], viewed: true, revisions: { 'src/a.rs': 'rev-a' } }));
  // A viewed file folds away; its lines are one click from open again.
  await waitFor(() => expect(screen.queryByText('Why u8?')).toBeNull());
});

it('reruns failed checks and resolves threads through the host capabilities', async () => {
  renderReview();
  await screen.findByRole('heading', { name: /Stream tokens/ });
  fireEvent.click(screen.getByRole('button', { name: 'Resolve' }));
  await waitFor(() => expect(calls.find(call => call.action === 'resolve')?.input).toEqual({ threadId: 't1', resolved: true }));
  fireEvent.click(screen.getByRole('tab', { name: /Checks/ }));
  fireEvent.click(await screen.findByRole('button', { name: 'Rerun failed jobs' }));
  await waitFor(() => expect(calls.find(call => call.action === 'rerun')?.input).toEqual({ token: 'run:1' }));
});

it('shows the stack, opens a layer and merges or rebases it with the loaded heads', async () => {
  const layer = (number: number, state: 'open' | 'merged', head: string, base: string) => ({ number, url: `https://github.com/fixture/parser/pull/${number}`, title: `Layer ${number}`, state, draft: false, headRef: `l${number}`, headSha: head, baseRef: base });
  current = { ...detail(), stack: { id: 's', native: false, base: 'main', layers: [layer(5, 'merged', 'h5', 'main'), layer(6, 'open', 'h6', 'l5'), layer(7, 'open', 'head-7', 'l6'), layer(8, 'open', 'h8', 'l7')] } };
  const opened: string[] = [];
  render(<MemoryRouter><PullRequestReview url="https://github.com/fixture/parser/pull/7" onOpenPull={next => opened.push(next)} /></MemoryRouter>);
  const bar = await screen.findByRole('navigation', { name: /Stack/ });
  expect(within(bar).getByRole('button', { name: /#7/ }).getAttribute('aria-current')).toBe('page');
  fireEvent.click(within(bar).getByRole('button', { name: /#8/ }));
  expect(opened).toEqual(['https://github.com/fixture/parser/pull/8']);
  fireEvent.click(within(bar).getByRole('button', { name: 'Merge stack' }));
  const dialog = await screen.findByRole('dialog');
  expect(within(dialog).getByText('#6')).toBeTruthy();
  expect(within(dialog).queryByText('#8')).toBeNull();
  fireEvent.click(within(dialog).getByRole('button', { name: 'Merge 2 PRs' }));
  await waitFor(() => expect(calls.find(call => call.action === 'mergeStack')?.input).toEqual({ method: 'merge', expectedHeads: [{ number: 6, headSha: 'h6' }, { number: 7, headSha: 'head-7' }] }));
  fireEvent.click(within(bar).getByRole('button', { name: 'Rebase stack' }));
  const confirm = await screen.findByRole('dialog');
  expect(within(confirm).getByText(/force-pushes/)).toBeTruthy();
  fireEvent.click(within(confirm).getByRole('button', { name: 'Rebase stack' }));
  await waitFor(() => expect(calls.find(call => call.action === 'rebaseStack')?.input).toEqual({ expectedHeads: [{ number: 6, headSha: 'h6' }, { number: 7, headSha: 'head-7' }, { number: 8, headSha: 'h8' }] }));
});
