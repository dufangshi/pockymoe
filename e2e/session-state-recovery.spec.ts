import {test,expect,type Page,type WebSocketRoute} from '@playwright/test';
import {randomUUID} from 'node:crypto';
import {mkdir} from 'node:fs/promises';
import path from 'node:path';
import {DatabaseSync} from 'node:sqlite';

const port=Number(process.env.E2E_API_PORT??8787);
const base=`http://127.0.0.1:${port}`;
const database=path.resolve(process.env.E2E_DATABASE_URL??`.local/e2e-${port}.sqlite`);

test('ACP live state repairs a stale snapshot and queued composer input survives reload',async({page,request})=>{
  const absPath=path.resolve('.local',`state-recovery-${randomUUID()}`);await mkdir(absPath,{recursive:true});
  const ws=await (await request.post(`${base}/api/workspaces`,{data:{absPath,label:'State recovery fixture'}})).json();
  const created=await (await request.post(`${base}/api/threads/start`,{data:{workspaceId:ws.id,title:'State recovery fixture',provider:'acp',agentId:'grok',model:'ios-e2e-stream',approvalMode:'yolo'}})).json();
  const id=created.id??created.thread.id;
  const detail=async()=>await (await request.get(`${base}/api/threads/${id}`)).json();
  try {
    await page.goto(`/threads/${id}`);
    await page.getByRole('textbox',{name:'Prompt'}).fill('Inspect this repository in depth');
    await page.getByRole('button',{name:'Send Prompt',exact:true}).click();
    await expect.poll(async()=>(await detail()).thread.status).toBe('running');
    const db=new DatabaseSync(database);
    try {
      // The fixture exists only in the explicitly isolated Playwright database.
      expect(db.prepare('SELECT id FROM threads WHERE id=?').get(id)).toBeTruthy();
      db.prepare("UPDATE threads SET status='interrupted',last_error='stale fixture' WHERE id=?").run(id);
      db.prepare("UPDATE thread_turns SET status='interrupted',completed_at='stale' WHERE thread_id=?").run(id);
    } finally {db.close();}
    await page.goto(`/threads/${id}`);
    await expect.poll(async()=>(await detail()).thread.status).toBe('running');
    await expect(page.getByRole('textbox',{name:'Prompt'})).toBeVisible();
    await expect(page.getByText('Interrupted by user',{exact:true})).toHaveCount(0);
    const prompt=`Keep this saved message ${randomUUID()}`;
    await page.getByRole('textbox',{name:'Prompt'}).fill(prompt);
    await page.getByRole('button',{name:'Send Prompt',exact:true}).click();
    await expect.poll(async()=>(await detail()).pendingSteers.map((p:{prompt:string})=>p.prompt)).toContain(prompt);
    await page.reload();
    await expect(page.getByText(prompt,{exact:true}).first()).toBeVisible();
    await request.post(`${base}/api/threads/${id}/interrupt`,{data:{}});
    await expect.poll(async()=>(await detail()).thread.status).toBe('idle');
    const completed=await detail();
    expect(completed.turns).toHaveLength(2);
    expect(completed.turns[1].status).toBe('completed');
    expect(completed.pendingSteers).toHaveLength(0);
    await page.reload();
    await expect(page.getByText(prompt,{exact:true}).first()).toBeVisible();
    expect((await detail()).turns[1].items.filter((i:{kind:string;text:string})=>i.kind==='userMessage'&&i.text===prompt)).toHaveLength(1);
  } finally {await request.post(`${base}/api/threads/${id}/interrupt`,{data:{}});}
});

// The working turn's text, without the fake runtime's unsaved plan card and timers.
const runningTurnText=(page:Page)=>page.evaluate(()=>{
  const turn=[...document.querySelectorAll('[data-timeline-turn]')].at(-1);
  if(!turn) return '';
  const copy=turn.cloneNode(true) as HTMLElement;copy.querySelectorAll('.tlog-plan').forEach(n=>n.remove());
  document.body.append(copy);const text=copy.innerText;copy.remove();
  return text.replace(/\d+(\.\d+)?\s?(s|ms|m)(?![a-z])/g,'#').replace(/\n+/g,'\n').trim();
});

for(const gap of ['lost connection','suspended tab'] as const) test(`a working turn shows the steps it missed during a ${gap} without a reload`,async({page,context,request})=>{
  const absPath=path.resolve('.local',`weak-network-${randomUUID()}`);await mkdir(absPath,{recursive:true});
  const ws=await (await request.post(`${base}/api/workspaces`,{data:{absPath,label:'Weak network'}})).json();
  const created=await (await request.post(`${base}/api/threads/start`,{data:{workspaceId:ws.id,title:'Weak network',provider:'acp',agentId:'codex',model:'ios-e2e-stream',approvalMode:'yolo'}})).json();
  const id=created.id??created.thread.id;
  const sockets:{socket:WebSocketRoute;dead:boolean}[]=[];
  await page.routeWebSocket(/\/ws(?:\?.*)?$/,socket=>{
    const entry={socket,dead:false};sockets.push(entry);
    const server=socket.connectToServer();
    socket.onMessage(m=>{if(!entry.dead) server.send(m);});
    server.onMessage(m=>{if(!entry.dead) socket.send(m);});
  });
  const setHidden=(hidden:boolean)=>page.evaluate(value=>{
    Object.defineProperty(document,'visibilityState',{configurable:true,get:()=>value?'hidden':'visible'});
    Object.defineProperty(document,'hidden',{configurable:true,get:()=>value});
    document.dispatchEvent(new Event('visibilitychange'));
  },hidden);
  try {
    await page.goto(`/threads/${id}`);
    await expect(page.getByRole('textbox',{name:'Prompt'})).toBeVisible();
    // Streams reads, commands and an edit for ~12 s, then holds a command open.
    await request.post(`${base}/api/threads/${id}/prompt`,{data:{prompt:'Split prompt-queue.ts into storage and ordering (live)'}});
    await expect(page.getByText("I'll start by reading the queue",{exact:false})).toBeVisible();
    const held=async()=>(await (await request.get(`${base}/api/threads/${id}?view=summary&limit=1`)).json()).turns.at(-1).deferredItemCount>=7;
    if(gap==='lost connection'){
      // The socket dies with the network; it reconnects while the turn still works.
      await context.setOffline(true);
      for(const s of sockets) s.socket.close({code:1006});
      await page.waitForTimeout(3500);
      await context.setOffline(false);
      await expect.poll(held,{timeout:30_000}).toBe(true);
    }else{
      // A phone tab in the background: its socket stops delivering without closing.
      await setHidden(true);
      for(const s of sockets) s.dead=true;
      await expect.poll(held,{timeout:30_000}).toBe(true);
      await setHidden(false);
    }
    const fresh=await context.newPage();
    await fresh.goto(`/threads/${id}`);
    await expect(fresh.getByText('sed -i',{exact:false}).first()).toBeVisible();
    const expected=await runningTurnText(fresh);
    // Well before the 30 s heartbeat replaces a socket that stopped answering.
    await expect.poll(()=>runningTurnText(page),{timeout:8_000}).toBe(expected);
  } finally {await request.post(`${base}/api/threads/${id}/interrupt`,{data:{}});}
});
