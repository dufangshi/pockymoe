import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {mkdir,writeFile} from 'node:fs/promises';
import {root,treerSource,read,api,until,deviceExec,run} from './common.mjs';

// Run against the already-running candidate fake hub; never replace the device
// container or the separately installed release App used by setup-e2e.mjs.
const evidenceDir=process.env.TREER_LAB_SYNC_EVIDENCE_DIR ?? `${root}/.scratch/treer-bidirectional`;
await mkdir(evidenceDir,{recursive:true,mode:0o700});
const state=await read(),browser=await read('browser.json');
assert.equal(state.authEnabled,true);
const ws=`/api/workspaces/${state.workspaceId}`;
const snap=await api(`${ws}/snapshot`);
for(const id of [browser.firstAgentId,browser.secondAgentId]) {
  const agent=snap.agents.find(a=>a.agent_id===id);
  assert.equal(agent?.interface?.port,18880,'Two bound fake-hub Agents are required');
}
const config=await api(`${ws}/agents/${browser.firstAgentId}/interface/ui/api/config/runtime`);
assert.equal(config.hostScope?.kind,'treerAgent','A candidate with scoped browser synchronization is required');
const machine=snap.servers.find(s=>s.name==='lab-device'&&s.status==='online');
assert.ok(machine);
const name=`lab-sync-driver-${randomUUID().slice(0,8)}`;
const jobDir=`/workspace/${name}-jobs`;
await deviceExec(['mkdir','-p',jobDir]);
let worker;
try {
  await api(`${ws}/agents`,{server_id:machine.server_id,name,kind:'command',cwd:'.',
    args:['/usr/bin/env',`TREER_LAB_JOB_DIR=${jobDir}`,'/usr/local/bin/node','/opt/lab/managed-driver.mjs']});
  worker=await until('scoped sync test worker',async()=>{
    try{return JSON.parse(await deviceExec(['cat',`${jobDir}/ready.json`]));}catch{return false;}
  });
  await run(process.execPath,[`${treerSource}/web/node_modules/@playwright/test/cli.js`,'test',
    'bidirectional.spec.mjs','--config',`${root}/scripts/treer-lab/playwright.config.mjs`,'--project=desktop-chromium'],
    {inherit:true,timeout:240000,env:{TREER_LAB_SYNC_JOB_DIR:jobDir,TREER_LAB_SYNC_EVIDENCE_DIR:evidenceDir}});
  await writeFile(`${evidenceDir}/sync-evidence.json`,JSON.stringify({result:'passed',
    browserProject:'desktop-chromium',spec:'bidirectional.spec.mjs',recordedAt:new Date().toISOString()},null,2)+'\n');
  console.log('PASS authenticated browser bidirectional synchronization and isolation');
} finally {
  if(worker) await api(`${ws}/agents/${worker.agentId}/stop`,{});
}
