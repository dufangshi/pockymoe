import {createRequire} from 'node:module';
import {appendFile,writeFile,mkdir} from 'node:fs/promises';
import {randomUUID} from 'node:crypto';
import {posix} from 'node:path';
import {origin,sessionCookie,treerSource,read,api,deviceExec,until,root} from './common.mjs';
const require=createRequire(`${treerSource}/web/package.json`);
const {test,expect}=require('@playwright/test');
const evidence=process.env.TREER_LAB_SYNC_EVIDENCE_DIR ?? `${root}/.scratch/treer-bidirectional`;
await mkdir(evidence,{recursive:true,mode:0o700});
const state=await read('browser.json');
const ws=`/api/workspaces/${state.workspaceId}`;
const jobDir=process.env.TREER_LAB_SYNC_JOB_DIR ?? '/workspace/lab-jobs';
const tunnel=id=>`${origin}${ws}/agents/${id}/interface/ui/`;
const snapshot=()=>api(`${ws}/snapshot`);
const hub=async(path,body)=>JSON.parse(await deviceExec(['curl','-fsS',...(body?['-H','content-type: application/json','-d',JSON.stringify(body)]:[]),`http://127.0.0.1:18880${path}`]));
async function managed(args){
 const id=randomUUID();
 await deviceExec(['sh','-c','umask 077; cat > "$1"','sh',`${jobDir}/${id}.request.json`],{input:JSON.stringify(args)});
 const r=await until('managed CLI reply',async()=>{try{return JSON.parse(await deviceExec(['cat',`${jobDir}/${id}.reply.json`]));}catch{return false;}},70000);
 expect(r.code,args.slice(0,3).join(' ')).toBe(0);return r.value;
}
async function auth(context){
 const cookie=(await sessionCookie()).split('; ').find(p=>p.startsWith('treer_session='));
 expect(Boolean(cookie)).toBe(true);
 await context.addCookies([{name:'treer_session',value:cookie.slice('treer_session='.length),url:origin,httpOnly:true,sameSite:'Lax'}]);
}
async function check(name,fn){console.log(`CHECK ${name}`);await test.step(name,fn);await appendFile(`${evidence}/checks.log`,`PASS ${name}\n`);}
async function threadAt(context,agent){
 const r=await context.request.get(tunnel(agent),{maxRedirects:0});expect(r.status()).toBe(307);
 return r.headers().location.slice('threads/'.length);
}
async function ready(page){await expect(page.locator('[contenteditable=true], [contenteditable=plaintext-only]').first()).toBeVisible();}
async function menu(page,title,action){
 await page.getByTestId('recent-chats').getByRole('button',{name:`Actions for ${title}`,exact:true}).click();
 await page.locator('.recent-thread-menu').getByRole('button',{name:new RegExp(`^${action}$`,'i')}).click();
}

test('authenticated bidirectional UI sync, tabs, split, storage and deletion',async({page,context})=>{
 test.setTimeout(180000);
 await auth(context);
 const firstBase=tunnel(state.firstAgentId),secondBase=tunnel(state.secondAgentId);
 const firstId=await threadAt(context,state.firstAgentId),secondId=await threadAt(context,state.secondAgentId);
 const first=(await hub(`/api/threads/${firstId}`)).thread,second=(await hub(`/api/threads/${secondId}`)).thread;
 const errors=[];page.on('response',r=>{if(r.status()>=400)errors.push({status:r.status(),path:new URL(r.url()).pathname});});
 page.on('pageerror',e=>errors.push({pageerror:e.message}));
 let created,newAgent,renamed,unboundId;
 try{
 await check('1 UI create binds a same-folder Treer Agent and opens its thread; prompt succeeds without 403',async()=>{
  await page.goto(firstBase);await ready(page);
  await page.locator('.matter-new-thread').click();
  const dialog=page.getByTestId('create-thread-dialog');
  const title=`Task G UI ${randomUUID().slice(0,8)}`;
  await dialog.locator('input[id$=thread-title]').fill(title);
  await dialog.locator('select[id$=thread-model]').selectOption({label:'GPT-5.4'});
  const response=page.waitForResponse(r=>r.request().method()==='POST' && new URL(r.url()).pathname===new URL(`${firstBase}api/threads/start`).pathname);
  await dialog.getByRole('button',{name:'Create Thread',exact:true}).click();
  const r=await response;expect(r.status()).toBe(200);created=await r.json();
  await expect(page).toHaveURL(`${firstBase}threads/${created.id}`);await ready(page);
  newAgent=await until('UI-created Agent binding',async()=>(await snapshot()).agents.find(a=>a.name.startsWith(title)&&a.interface));
  const a=(await snapshot()).agents.find(a=>a.agent_id===state.firstAgentId);
  expect(typeof a.path).toBe('string');expect(typeof newAgent.path).toBe('string');expect(posix.dirname(newAgent.path)).toBe(posix.dirname(a.path));expect(newAgent.name).toBe(`${title}-${created.id}`);expect(newAgent.cwd).toBe(a.cwd);expect(newAgent.interface.port).toBe(18880);
  expect(await threadAt(context,newAgent.agent_id)).toBe(created.id);
  const prompt='Task G browser prompt';await page.locator('[contenteditable=true], [contenteditable=plaintext-only]').first().fill(prompt);await page.getByRole('button',{name:'Send Prompt',exact:true}).click();
  await expect(page.getByText(`ok: ${prompt}`,{exact:true}).first()).toBeVisible({timeout:20000});
  expect(errors).toEqual([]);
  await writeFile(`${evidence}/created.json`,JSON.stringify({thread:created,agent:newAgent},null,2));
 });
 await check('2 UI rename updates the Treer Agent label',async()=>{
  renamed=`Task G renamed ${randomUUID().slice(0,8)}`;
  await menu(page,created.title,'Rename thread');
  const d=page.getByRole('dialog',{name:/^Rename thread$/i});
  await d.locator('input').fill(renamed);await d.getByRole('button',{name:'Save',exact:true}).click();await expect(d).toBeHidden();
  await expect.poll(async()=>(await snapshot()).agents.find(a=>a.agent_id===newAgent.agent_id)?.name).toBe(`${renamed}-${created.id}`);
  expect((await hub(`/api/threads/${created.id}`)).thread.title).toBe(renamed);
 });
 await check('3 Agent A lists and opens B; tabs and split remain writable',async()=>{
  const list=await context.request.get(`${firstBase}api/threads`);expect(list.status()).toBe(200);expect((await list.json()).some(t=>t.id===secondId)).toBe(true);
  await page.locator(`.matter-thread-tabs a[href$="/threads/${secondId}"]`).click();await expect(page).toHaveURL(`${firstBase}threads/${secondId}`);await ready(page);
  await page.locator(`.matter-thread-tabs a[href$="/threads/${created.id}"]`).click();await expect(page).toHaveURL(`${firstBase}threads/${created.id}`);
  await page.getByTestId('workbench-split-trigger').click();
  await page.getByTestId('workbench-thread-picker').locator(`[data-thread-id="${secondId}"]`).click();
  await expect(page.getByTestId('reference-pane')).toBeVisible();
  await expect(page.getByTestId('reference-pane').locator('[contenteditable=true], [contenteditable=plaintext-only]')).toBeVisible();
  const left=page.getByTestId('primary-pane').getByRole('textbox',{name:'Prompt',exact:true});
  const right=page.getByTestId('reference-pane').getByRole('textbox',{name:'Prompt',exact:true});
  await left.fill('Task G primary draft');await right.fill('Task G reference draft');
  await expect(left).toHaveText('Task G primary draft');await expect(right).toHaveText('Task G reference draft');
  await left.fill('');await right.fill('');
  expect(errors).toEqual([]);
 });
 await check('5 favorites remain openable across pages and stale recents/favorites disappear silently',async()=>{
  await menu(page,renamed,'Star thread');await expect(page.getByTestId('shortcuts')).toContainText(renamed);
  const other=await context.newPage();await other.goto(secondBase);await ready(other);
  await expect(other.getByTestId('shortcuts')).not.toContainText(renamed);
  await expect(other.getByTestId('recent-chats')).not.toContainText(renamed);
  const keys=await page.evaluate(()=>Object.keys(localStorage));const nav=keys.find(k=>k.startsWith('remote-codex.local-workbench.v1:'));
  expect(nav).toBeTruthy();
  const unbound=await hub('/api/threads/start',{workspaceId:first.workspaceId,provider:'codex',model:'gpt-5.4',title:'Task G hidden unbound'});
  unboundId=unbound.id;await writeFile(`${evidence}/unbound.json`,JSON.stringify(unbound));
  const denied=await context.request.get(`${firstBase}api/threads/${unbound.id}`);expect(denied.status()).toBe(403);
  const deniedPage=await context.request.get(`${firstBase}threads/${unbound.id}`);expect(deniedPage.status()).toBe(403);
  const list=await context.request.get(`${firstBase}api/threads`);expect((await list.json()).some(t=>t.id===unbound.id)).toBe(false);
  await page.evaluate(({nav,id})=>{const v=JSON.parse(localStorage.getItem(nav));v.threads.push({deviceId:null,threadId:id,title:'STALE_UNBOUND_FAVORITE',workspaceLabel:'project',favorite:true,visitedAt:new Date().toISOString()},{deviceId:null,threadId:'gone-task-g',title:'STALE_DELETED_FAVORITE',workspaceLabel:'project',favorite:true,visitedAt:new Date().toISOString()});localStorage.setItem(nav,JSON.stringify(v));const members=Object.keys(localStorage).find(k=>k.endsWith('.members'));if(!members)throw new Error('Split membership storage was not created');localStorage.setItem(members,JSON.stringify({schemaVersion:1,referenceId:id,referenceDeviceId:null}));},{nav,id:unbound.id});
  await page.reload();await ready(page);
  await expect.poll(()=>page.evaluate(nav=>JSON.parse(localStorage.getItem(nav)).threads.some(t=>t.title.startsWith('STALE_')),nav)).toBe(false);
  await expect(page.locator('body')).not.toContainText('无法解析此线程');await expect(page.locator('body')).not.toContainText('STALE_');await expect(page.getByTestId('reference-pane')).toBeHidden();
  await expect(page.getByTestId('workbench-thread-picker')).toBeHidden();
  await page.getByTestId('workbench-split-trigger').click();await expect(page.getByTestId('workbench-thread-picker')).not.toContainText(unbound.title);await page.keyboard.press('Escape');
  await other.close();
 });
 await check('7 Treer Launch-profile Agent becomes visible and opens in Agent A UI',async()=>{
  const profiles=await managed(['agent','admin','profile','list']);
  const profilesList=Array.isArray(profiles)?profiles:profiles.profiles;
  const profile=profilesList.find(p=>p.name.startsWith('lab-attach-'));
  expect(profile).toBeTruthy();const name=`task-g-launch-${randomUUID().slice(0,8)}`;
  await managed(['agent','admin','profile','launch',profile.id??profile.profile_id??profile.name,'--machine',newAgent.server_id,'--name',name]);
  const launched=await until('Launch interface',async()=>(await snapshot()).agents.find(a=>a.name===name&&a.interface));
  const id=await threadAt(context,launched.agent_id);
  await expect.poll(async()=>{const r=await context.request.get(`${firstBase}api/threads`);return (await r.json()).some(t=>t.id===id);}).toBe(true);
  const t=(await hub(`/api/threads/${id}`)).thread;
  await page.locator(`.matter-thread-tabs a[href$="/threads/${id}"]`).click();await expect(page).toHaveURL(`${firstBase}threads/${id}`);await ready(page);
  await writeFile(`${evidence}/launched.json`,JSON.stringify({agent:launched,threadId:id},null,2));
 });
 await check('4 deleting another bound thread removes its Agent and history',async()=>{
  await menu(page,renamed,'Delete thread');const d=page.getByRole('dialog',{name:/^Delete thread\?$/i});await d.getByRole('button',{name:'Delete',exact:true}).click();await expect(d).toBeHidden();
  await expect.poll(async()=>(await snapshot()).agents.some(a=>a.agent_id===newAgent.agent_id)).toBe(false);
  const r=await context.request.get(`${firstBase}api/threads/${created.id}`);expect([403,404]).toContain(r.status());
  expect((await hub('/api/threads')).some(t=>t.id===created.id)).toBe(false);
  expect(errors.filter(e=>e.pageerror || (e.status>=400&&!e.path.includes('gone-task-g')&&!e.path.includes(unboundId)))).toEqual([]);
 });
 }finally{await writeFile(`${evidence}/browser-responses.json`,JSON.stringify(errors,null,2));await writeFile(`${evidence}/last-page.txt`,await page.locator('body').innerText());}
});

test('authenticated tunnel denies management and unbound threads; AIS stays bound-only',async({context,page})=>{
 await check('6 authenticated tunnel preserves management/settings/device and AIS isolation',async()=>{
  const base=tunnel(state.firstAgentId);
  const anon=await context.request.get(base,{headers:{Origin:origin}});expect(anon.status()).toBe(401);
  await auth(context);
  for(const path of ['api/device/processes','api/device/metrics','api/config/workspace-settings','api/management/supervisor','api/management/upstreams','settings','devices','v1/transcript']){
   const r=await context.request.get(`${base}${path}`);expect(r.status(),path).toBe(403);
  }
  for(const [path,method] of [['api/config/workspace-settings','PATCH'],['api/config/model-pricing','PATCH'],['api/management/supervisor/check','POST'],['api/management/supervisor/update','POST'],['api/agent-runtimes/codex/install','POST']]){
   const r=await context.request.fetch(`${base}${path}`,{method,data:{}});expect(r.status(),path).toBe(403);
  }
  const firstId=await threadAt(context,state.firstAgentId),secondId=await threadAt(context,state.secondAgentId);
  const a=(await snapshot()).agents.find(a=>a.agent_id===state.firstAgentId);
  const controller=async(path,instance=a.interface.instance_id)=>{
   const output=await deviceExec(['curl','-sS','-H','Host: agent-interface','-H','x-treer-interface-source: controller-api','-H',`x-treer-agent-id: ${a.agent_id}`,'-H',`x-treer-interface-instance: ${instance}`,'-w','\n%{http_code}',`http://127.0.0.1:18880${path}`]);
   const i=output.lastIndexOf('\n');return {status:Number(output.slice(i+1)),body:JSON.parse(output.slice(0,i))};
  };
  const history=await controller(`/v1/transcript?limit=1000&threadId=${secondId}`);expect(history.status).toBe(200);expect(history.body.agent_id).toBe(a.agent_id);expect(history.body.entries.some(e=>e.content==='Treer lab first turn')).toBe(true);
  expect((await controller(`/api/threads/${secondId}`)).status).toBe(403);
  expect((await controller('/v1/status','wrong-instance')).status).toBe(403);
  const own=await managed(['agent','transcript',a.agent_id,'--limit','1000']);expect(own.agent_id).toBe(a.agent_id);expect(own.entries.some(e=>e.content==='Treer lab first turn')).toBe(true);
  await page.goto(`${base}settings`);await expect(page.locator('body')).not.toContainText('Check for updates');
  await writeFile(`${evidence}/security.json`,JSON.stringify({deniedRoutes:13,aisThread:firstId,otherThread:secondId,anonymous:401},null,2));
 });
});

test('UI deletion of the entry Agent own thread removes Agent and history',async({page,context})=>{
 test.setTimeout(60000);await auth(context);
 await check('4 own-thread deletion is allowed after confirmation and revokes its tunnel',async()=>{
  const snap=await snapshot(),machine=snap.servers.find(s=>s.name==='lab-device');
  const name=`task-g-self-${randomUUID().slice(0,8)}`;
  await api(`${ws}/agents`,{server_id:machine.server_id,name,kind:'command',cwd:'project',args:['/usr/local/bin/pockymoe','attach','--provider','codex','--model','gpt-5.4','--title',name,'--hub','http://127.0.0.1:18880','--hub-token-file','/workspace/hub/treer-attach.token']});
  const a=await until('self-delete fixture binding',async()=>(await snapshot()).agents.find(a=>a.name===name&&a.interface));
  const id=await threadAt(context,a.agent_id);await page.goto(tunnel(a.agent_id));await ready(page);
  await menu(page,name,'Delete thread');const d=page.getByRole('dialog',{name:/^Delete thread\?$/i});
  const response=page.waitForResponse(r=>r.request().method()==='DELETE'&&r.url().includes(id));
  await d.getByRole('button',{name:'Delete',exact:true}).click();expect((await response).ok()).toBe(true);
  await expect.poll(async()=>(await snapshot()).agents.some(x=>x.agent_id===a.agent_id)).toBe(false);
  expect((await hub('/api/threads')).some(t=>t.id===id)).toBe(false);
  const r=await context.request.get(tunnel(a.agent_id));expect([403,404,409]).toContain(r.status());
 });
});

test('attach PTY shows waiting, setup progress and an actionable provider error',async()=>{
 test.setTimeout(60000);
 await check('8 attach PTY reports waiting/progress and a readable provider error',async()=>{
  const machine=(await snapshot()).servers.find(s=>s.name==='lab-device');
  const suffix=randomUUID().slice(0,8),name=`lab-sync-error-${suffix}`;
  const data=`/workspace/${name}`,port=18881,appName=`lab-sync-error-hub-${suffix}`;
  await deviceExec(['mkdir','-p',data]);
  await api(`${ws}/agents`,{server_id:machine.server_id,name,kind:'command',cwd:'project',
   args:['/usr/local/bin/pockymoe','attach','--provider','claude','--hub',`http://127.0.0.1:${port}`,
    '--hub-token-file',`${data}/treer-attach.token`]});
  const agent=await until('error diagnostic Agent',async()=>(await snapshot()).agents.find(a=>a.name===name));
  const output=()=>api(`${ws}/agents/${agent.agent_id}/output?lines=100`);
  let appCreated=false;
  try {
   await until('attach waiting PTY',async()=>(await output()).text.includes('waiting for hub token'));
   // The temporary diagnostic hub has only Codex enabled. It never uses or
   // changes the fake lab hub or the setup-created release installation.
   await managed(['app','create','--machine',machine.server_id,'--name',appName,'--cwd','.',
    '--port',String(port),'/usr/bin/env','--','POCKYMOE_MODE=local','POCKYMOE_HOST_MODE=hosted',
    'POCKYMOE_HOSTED_BY=treer','POCKYMOE_E2E_FAKE_RUNTIME=1','POCKYMOE_ENABLED_AGENT_PROVIDERS=codex',
    'HOST=127.0.0.1',`PORT=${port}`,`POCKYMOE_DATABASE_PATH=${data}/supervisor.sqlite`,
    'POCKYMOE_WORKSPACE_ROOT=/workspace/project','POCKYMOE_WEB_DIST_DIR=/opt/pockymoe-web',
    '/usr/local/bin/pockymoe','supervisor']);
   appCreated=true;
   const result=await until('readable attach failure PTY',async()=>{
    const out=await output();return out.text.includes('Hub rejected attach binding')&&out;
   });
   expect(result.text).toContain('waiting for hub token');
   expect(result.text).toContain('waiting for hub reply (thread setup may take a few minutes)');
   expect(result.text).toContain('[provider_unavailable]: claude is not enabled on this hub.');
   await expect.poll(async()=>(await snapshot()).agents.find(a=>a.agent_id===agent.agent_id)?.status).toBe('exited');
   await writeFile(`${evidence}/attach-pty.json`,JSON.stringify(result,null,2));
  } finally {
   if(appCreated)await managed(['app','delete',`/${appName}`]);
   await managed(['agent','admin','delete',agent.agent_id]);
  }
 });
});
