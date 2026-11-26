import { beforeEach, expect, it, vi } from 'vitest';
import { loadExportSnapshot } from './transcriptExport';
import { downloadThreadImage, fetchThreadDetail, fetchThreadTurnDetail } from './api';

vi.mock('./api', () => ({ downloadThreadImage: vi.fn(), fetchThreadDetail: vi.fn(), fetchThreadTurnDetail: vi.fn() }));
beforeEach(() => vi.clearAllMocks());

it('hydrates deferred messages and embeds each attachment through the authenticated download API', async () => {
  vi.mocked(fetchThreadDetail).mockResolvedValue({ thread: { title: 'Photo review' }, totalTurnCount: 1,
    turns: [{ id: 'turn', hasDeferredItems: true, items: [] }] } as never);
  vi.mocked(fetchThreadTurnDetail).mockResolvedValue({ id: 'turn', status: 'completed', items: [
    { id: 'prompt', kind: 'userMessage', text: '[PHOTO ./photo.png] [PHOTO ./photo.png]' },
    { id: 'reply', kind: 'agentMessage', text: 'The image looks good.' },
  ] } as never);
  vi.mocked(downloadThreadImage).mockResolvedValue(new Blob(['image'], { type: 'image/png' }));
  const snapshot = await loadExportSnapshot('thread', { format: 'html', mode: 'latest', limit: 10 });
  expect(fetchThreadTurnDetail).toHaveBeenCalledWith('thread', 'turn');
  expect(downloadThreadImage).toHaveBeenCalledExactlyOnceWith('thread', './photo.png');
  expect(snapshot.images?.['./photo.png']).toBe('data:image/png;base64,aW1hZ2U=');
  expect(snapshot.turns[0]?.messages.map(m => m.text)).toContain('The image looks good.');
});

it('does not download PHOTO syntax examples in assistant replies as attachments', async () => {
  const reply = 'Use `[PHOTO …]` or `[PHOTO ./.temp/threads/…/image.png]` to describe the syntax.';
  vi.mocked(fetchThreadDetail).mockResolvedValue({ thread: { title: 'Photo syntax' }, totalTurnCount: 1,
    turns: [{ id: 'turn', status: 'completed', items: [
      { id: 'prompt', kind: 'userMessage', text: 'Explain photos [PHOTO ./real.png]' },
      { id: 'reply', kind: 'agentMessage', text: reply },
    ] }] } as never);
  vi.mocked(downloadThreadImage).mockResolvedValue(new Blob(['image'], { type: 'image/png' }));
  const snapshot = await loadExportSnapshot('thread', { mode: 'latest', limit: 10 });
  expect(downloadThreadImage).toHaveBeenCalledExactlyOnceWith('thread', './real.png');
  expect(Object.keys(snapshot.images!)).toEqual(['./real.png']);
  expect(snapshot.turns[0]?.messages[1]?.text).toBe(reply);
});

it.each(['turn',null])('exports every standalone switch anchored at %s', async (anchor) => {
  const switches = ['one','two'].map(id => ({id,kind:'harnessSwitch',text:'switch',afterTurnId:anchor,harnessSwitch:{fromAgentId:'codex',agentId:'claude',turns:1}}));
  vi.mocked(fetchThreadDetail).mockResolvedValue({thread:{title:'Switches'},turns:anchor ? [{id:'turn',status:'completed',items:[]}] : [],harnessSwitches:switches} as never);
  const snapshot=await loadExportSnapshot('thread',{mode:'latest',limit:10});
  expect(snapshot.harnessSwitches).toEqual(switches);
  expect(snapshot.turnCount).toBe(anchor ? 1 : 0);
});
