import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ThreadDto } from '@pockymoe/shared';
import { changeControl } from '../test/formControls';
import { HarnessSwitchFields } from './HarnessSwitchFields';
const api=vi.hoisted(()=>({deviceRequest:vi.fn(),interruptThread:vi.fn()}));
vi.mock('../lib/api',()=>api);
const thread={id:'thread',agentId:'codex',status:'idle'} as ThreadDto;
afterEach(cleanup);
beforeEach(()=>{
 vi.clearAllMocks();
 api.interruptThread.mockResolvedValue({});
 api.deviceRequest.mockImplementation(async (_device:string,path:string)=>{
  if(path.endsWith('/agents'))return[{id:'claude',displayName:'Claude'}];
  if(path.includes('/models?'))return[{id:'fable',model:'fable',displayName:'Fable',isDefault:true}];
  if(path.endsWith('/fork-turns'))return[{turnId:'turn-1',turnIndex:1}];
  if(path.endsWith('/preview'))return{text:'Saved history',turns:1,estimatedTokens:50,resumesSession:false};
  if(path.endsWith('/fork'))return{id:'fork'};
  return {};
 });
});
async function choose(){
 await waitFor(()=>expect(api.deviceRequest).toHaveBeenCalled());
 changeControl(screen.getByRole('combobox',{name:'Target harness'}),{target:{value:'claude'}});
 await waitFor(()=>expect(screen.getByRole('button',{name:'Preview handoff'})).not.toBeDisabled());
}
describe('harness handoff controls',()=>{
 it('previews before switching on the selected device',async()=>{
  const done=vi.fn();render(<HarnessSwitchFields thread={thread} deviceId="device-2" onSwitched={done}/>);await choose();
  fireEvent.click(screen.getByRole('button',{name:'Preview handoff'}));
  await screen.findByText('1 turn · ≈50 tokens');
  expect(api.deviceRequest.mock.calls.some(c=>String(c[1]).endsWith('/switch'))).toBe(false);
  fireEvent.click(screen.getByRole('button',{name:'Switch'}));await waitFor(()=>expect(done).toHaveBeenCalled());
  expect(api.deviceRequest).toHaveBeenCalledWith('device-2','api/threads/thread/harness/switch',expect.objectContaining({method:'POST'}));
 });
 it('forks through the selected turn with a foreign harness',async()=>{
  const done=vi.fn();render(<HarnessSwitchFields thread={thread} onSwitched={done}/>);await choose();
  changeControl(screen.getByRole('combobox',{name:'Handoff action'}),{target:{value:'fork'}});
  changeControl(screen.getByRole('combobox',{name:'Fork turn'}),{target:{value:'turn-1'}});
  fireEvent.click(screen.getByRole('button',{name:'Preview handoff'}));await screen.findByText('1 turn · ≈50 tokens');
  fireEvent.click(screen.getByRole('button',{name:'Fork'}));await waitFor(()=>expect(done).toHaveBeenCalledWith({id:'fork'}));
  const call=api.deviceRequest.mock.calls.find(c=>String(c[1]).endsWith('/fork'))!;
  expect(JSON.parse(call[2].body)).toEqual({turnId:'turn-1',target:{provider:'acp',agentId:'claude',model:'fable',reasoningEffort:null}});
 });
 it('settles an interrupted turn before switching',async()=>{
  const done=vi.fn();render(<HarnessSwitchFields thread={{...thread,status:'running'}} onSwitched={done}/>);await choose();
  fireEvent.click(screen.getByRole('button',{name:'Preview handoff'}));await screen.findByText('1 turn · ≈50 tokens');
  fireEvent.click(screen.getByRole('button',{name:'Stop and switch'}));await waitFor(()=>expect(done).toHaveBeenCalled());
  expect(api.interruptThread).toHaveBeenCalledWith('thread',{},undefined);
  const paths=api.deviceRequest.mock.calls.map(c=>String(c[1]));expect(paths.indexOf('api/threads/thread/release')).toBeLessThan(paths.indexOf('api/threads/thread/harness/switch'));
 });
});
