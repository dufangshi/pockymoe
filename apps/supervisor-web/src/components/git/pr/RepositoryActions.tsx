import { useState } from 'react';
import { ErrorMark, PathChip } from '@pockymoe/thread-ui';
import { request } from '../../../lib/api';
import { sourceAction } from './api';
import { prText as t } from './text';
export function CloneRepository({apiRoot}:{apiRoot:string}) {
  const [url,setUrl]=useState('');const [busy,setBusy]=useState(false);const [error,setError]=useState('');const [path,setPath]=useState('');
  return <form className="pr-form" onSubmit={e=>{e.preventDefault();setBusy(true);setError('');void request<{jobId:string}>(`${apiRoot}/source-control/clone`,{method:'POST',body:JSON.stringify({url})}).then(async({jobId})=>{
    for(let attempt=0;attempt<150;attempt++) {
      const job=await request<{status:string;error?:string;result?:{workspace:{absPath:string}}}>(`${apiRoot}/source-control/jobs/${encodeURIComponent(jobId)}`);
      if(job.status==='completed'){setPath(job.result?.workspace.absPath||'');return;}
      if(job.status==='failed')throw new Error(job.error);
      await new Promise(resolve=>setTimeout(resolve,2000));
    }
    throw new Error(t('loading'));
  }).catch(e=>setError(String(e))).finally(()=>setBusy(false));}}>
    <strong>{t('clone')}</strong><label>{t('gitUrl')}<input type="text" required value={url} onChange={e=>setUrl(e.target.value)}/></label><button className="pr-primary" disabled={busy||!url}>{busy?t('loading'):t('clone')}</button>{error&&<ErrorMark message={error}/>} {path&&<PathChip path={path}/>}
  </form>;
}
export function PublishRepository({threadId,deviceId}:{threadId:string;deviceId?:string|null|undefined}) {
  const [name,setName]=useState('');const [host,setHost]=useState('github.com');const [privateRepo,setPrivate]=useState(true);const [busy,setBusy]=useState(false);const [error,setError]=useState('');const [done,setDone]=useState(false);
  return <details><summary>{t('publish')}</summary><form className="pr-form" onSubmit={e=>{e.preventDefault();setBusy(true);setError('');void sourceAction(threadId,'publish',{name,host,private:privateRepo},deviceId).then(()=>setDone(true)).catch(e=>setError(String(e))).finally(()=>setBusy(false));}}>
    <label>{t('repositoryName')}<input required value={name} onChange={e=>setName(e.target.value)}/></label><label>{t('host')}<input required value={host} onChange={e=>setHost(e.target.value)}/></label><label className="pr-checkbox"><input type="checkbox" checked={privateRepo} onChange={e=>setPrivate(e.target.checked)}/>{t('private')}</label><button className="pr-primary" disabled={busy||done}>{t('publish')}</button>{error&&<ErrorMark message={error}/>} {done&&<p role="status">{t('result')}</p>}
  </form></details>;
}
