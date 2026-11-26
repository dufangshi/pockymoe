import {fireEvent,render,screen,waitFor} from '@testing-library/react';
import {beforeEach,expect,it,vi} from 'vitest';
import {PullRequestsPanel} from './PullRequestsPanel';
const request=vi.hoisted(()=>vi.fn());const action=vi.hoisted(()=>vi.fn());
vi.mock('../../../lib/api',()=>({request}));
vi.mock('./api',()=>({sourceRoot:()=> 'api',sourceAction:action}));
vi.mock('@pockymoe/thread-ui/i18n',()=>({useI18n:()=>{},getLocale:()=> 'en'}));
vi.mock('@pockymoe/thread-ui',()=>({Select:()=>null,PathChip:()=>null,ErrorMark:({message}:{message:string})=><p>{message}</p>,confirmAsync:()=>Promise.resolve(true)}));
beforeEach(()=>{request.mockReset();action.mockReset();request.mockResolvedValue({links:[{url:'https://github.com/a/b/pull/42',number:42,kind:'pr',repository:{owner:'a',name:'b',host:'github.com',kind:'github'}}]});action.mockImplementation(async(_thread:string,op:string)=>op==='show'?{data:{title:'Review me',head:{sha:'reviewed-head'},state:'open'}}:op==='watchStatus'?{state:{active:false}}:{data:{}});});
it('merges only the displayed head and keeps its thread, working copy and linked URL',async()=>{
 render(<PullRequestsPanel threadId="thread" workingCopyId="copy" deviceId="device"/>);
 fireEvent.click(await screen.findByRole('button',{name:'a/b · PR #42'}));
 await screen.findByDisplayValue('Review me');
 fireEvent.click(screen.getByRole('button',{name:/^Merge$/}));
 await waitFor(()=>expect(action).toHaveBeenCalledWith('thread','merge',expect.objectContaining({number:42,expectedHead:'reviewed-head',workingCopyId:'copy',url:'https://github.com/a/b/pull/42'}),'device'));
});
it('preserves comment drafts after a failed host request',async()=>{
 render(<PullRequestsPanel threadId="thread"/>);fireEvent.click(await screen.findByRole('button',{name:'a/b · PR #42'}));await screen.findByDisplayValue('Review me');
 action.mockRejectedValueOnce(new Error('Host unavailable'));
 fireEvent.change(screen.getByLabelText('Comment'),{target:{value:'Keep this draft'}});fireEvent.click(screen.getByRole('button',{name:/^Comment$/}));
 await screen.findByText('Error: Host unavailable');expect(screen.getByDisplayValue('Keep this draft')).toBeTruthy();
});
it('shows setup failure and output while retaining the created worktree result',async()=>{
 render(<PullRequestsPanel threadId="thread"/>);fireEvent.click(await screen.findByRole('button',{name:'a/b · PR #42'}));await screen.findByDisplayValue('Review me');
 action.mockResolvedValueOnce({workingCopy:{path:'/tmp/new-copy'},setup:{completed:false,error:'Setup command 1 failed',output:['Installing dependencies','Missing local fixture']}});
 fireEvent.click(screen.getByRole('button',{name:'Create worktree'}));
 await screen.findByText('Worktree created; setup failed');
 expect(screen.getByText('Setup command 1 failed')).toBeTruthy();
 expect(screen.getByText(/Installing dependencies/)).toBeTruthy();
 expect(screen.getByText(/Missing local fixture/)).toBeTruthy();
});
it('shows merged state when the host closes a successfully merged PR',async()=>{
 action.mockImplementation(async(_thread:string,op:string)=>op==='show'?{data:{title:'Already merged',head:{sha:'merged-head'},state:'closed',merged:true}}:op==='watchStatus'?{state:{active:false}}:{data:{}});
 render(<PullRequestsPanel threadId="thread"/>);
 fireEvent.click(await screen.findByRole('button',{name:'a/b · PR #42'}));
 await screen.findByText('State: Merged');
});
