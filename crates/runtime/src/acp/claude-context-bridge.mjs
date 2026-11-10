// Thin extension of the installed ACP adapter. Conversation ownership stays in ACP.
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';
import { join } from 'node:path';
const directory=process.argv[2];
const require=createRequire(join(directory,'package.json'));
// The Supervisor names the module: releases moved it (dist/acp-agent.js, then dist/v1/).
const { v1AgentApp }=await import(pathToFileURL(join(directory,process.argv[3]??'dist/acp-agent.js')).href);
const { applyManagedPolicyEnv }=await import(pathToFileURL(join(directory,'dist/managed-policy.js')).href);
const { nodeToWebReadable,nodeToWebWritable }=await import(pathToFileURL(join(directory,'dist/utils.js')).href);
const { ndJsonStream }=await import(pathToFileURL(require.resolve('@agentclientprotocol/sdk')).href);
await applyManagedPolicyEnv();
console.log=console.error;console.info=console.error;console.warn=console.error;console.debug=console.error;
let agent;
const inFlight=new Map();
const app=v1AgentApp(undefined,created=>{agent=created;}).onRequest('_pockymoe/session/context',{
  parse(value){
    if(!value||typeof value.sessionId!=='string'||!['summary','full'].includes(value.detail))throw new Error('Invalid context request');
    return value;
  },
},async ({params})=>{
  const session=agent?.sessions[params.sessionId];
  if(!session?.query?.getContextUsage)throw new Error('Context inspection is unavailable for this loaded session');
  const key=params.sessionId+':'+params.detail;
  if(!inFlight.has(key)){
    const query=session.query;
    const promise=query.getContextUsage({detail:params.detail}).then(value=>{
      if(agent.sessions[params.sessionId]?.query!==query)throw new Error('Context session changed');
      return value;
    }).finally(()=>inFlight.delete(key));
    inFlight.set(key,promise);
  }
  return inFlight.get(key);
});
const {closed}=app.connect(ndJsonStream(nodeToWebWritable(process.stdout),nodeToWebReadable(process.stdin)));
let closing=false;
async function shutdown(){if(closing)return;closing=true;await agent?.dispose().catch(()=>{});process.exit(0);}
closed.then(shutdown);process.on('SIGTERM',shutdown);process.on('SIGINT',shutdown);process.stdin.resume();
