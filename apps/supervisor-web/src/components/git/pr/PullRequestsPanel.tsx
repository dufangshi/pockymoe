import { PublishRepository } from './RepositoryActions';
import { useCallback, useEffect, useState } from 'react';
import { ExternalLink, RefreshCw, X } from 'lucide-react';
import { Select, confirmAsync, ErrorMark, PathChip } from '@pockymoe/thread-ui';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { request } from '../../../lib/api';
import { sourceAction, sourceRoot, type ItemLink } from './api';
import { prText as t } from './text';
import './pr.css';
type Data = Record<string, unknown>;
function string(value:unknown) {return typeof value === 'string' ? value : '';}
function headOid(data:Data) { return string((data.head as Data)?.sha) || string(data.sha) || string(((data.source as Data)?.commit as Data)?.hash) || string((data.lastMergeSourceCommit as Data)?.commitId); }
export function PullRequestsPanel({threadId,workingCopyId,deviceId}:{threadId:string;workingCopyId?:string|undefined;deviceId?:string|null|undefined}) {
  useI18n();
  const [links,setLinks]=useState<ItemLink[]>([]); const [url,setUrl]=useState(''); const [error,setError]=useState(''); const [busy,setBusy]=useState(false);
  const [itemKind,setItemKind]=useState('pr');
  const [creating,setCreating]=useState(false);const [title,setTitle]=useState('');const [body,setBody]=useState('');const [head,setHead]=useState('');const [base,setBase]=useState('main');const [draft,setDraft]=useState(true);
  const [selected,setSelected]=useState<ItemLink|null>(null);const [detail,setDetail]=useState<Data|null>(null);const [extra,setExtra]=useState<unknown>(null);const [comment,setComment]=useState('');const [method,setMethod]=useState('squash');const [watching,setWatching]=useState(false);
  const reload=useCallback(async()=>{const result=await request<{links:ItemLink[]}>(`${sourceRoot(deviceId)}/threads/${encodeURIComponent(threadId)}/pull-requests`);setLinks(result.links);},[deviceId,threadId]);
  useEffect(()=>{let alive=true;void reload().catch(e=>{if(alive)setError(String(e));});return()=>{alive=false;};},[reload]);
  const run=async(action:string,input:Record<string,unknown>={})=>{setBusy(true);setError('');try {const result=await sourceAction<Data>(threadId,action,{workingCopyId,...input},deviceId);await reload();return result;}catch(e){setError(String(e));return null;}finally{setBusy(false);}};
  const open=async(link:ItemLink)=>{setSelected(link);setExtra(null);setDetail(null);const result=await run('show',{number:link.number,kind:link.kind,url:link.url});if(result){const data=result.data as Data;setDetail(data);setTitle(string(data.title));setBody(string(data.body)||string(data.description));const watch=await run('watchStatus');setWatching(Boolean((watch?.state as Data)?.active));}};
  const itemInput=()=>({number:selected?.number,kind:selected?.kind,url:selected?.url});
  return <section className="pr-panel" aria-label={t('title')}>
    <div className="pr-toolbar"><strong>{t('title')}</strong><button type="button" disabled={busy} onClick={()=>{void run('scan').then(v=>{if(v)setExtra({repository:v.repository,authenticated:v.authenticated});});}} title={t('refresh')} aria-label={t('refresh')}><RefreshCw size={16}/></button><button type="button" onClick={()=>{setCreating(!creating);setSelected(null);setTitle('');setBody('');}}>{creating?t('cancel'):t('newPr')}</button></div>
    <div className="pr-toolbar"><Select aria-label={t('kind')} value={itemKind} onValueChange={setItemKind} options={[{value:'pr',label:t('title')},{value:'issue',label:t('issues')}]}/><button disabled={busy} onClick={()=>{void run('list',{kind:itemKind}).then(v=>{if(v)setExtra(v.data);});}}>{t('browse')}</button></div>
    {error&&<ErrorMark message={error}/>}
    <form className="pr-toolbar" onSubmit={e=>{e.preventDefault();void run('link',{url}).then(v=>{if(v)setUrl('');});}}><input aria-label={t('url')} placeholder={t('url')} type="url" required value={url} onChange={e=>setUrl(e.target.value)}/><button disabled={busy}>{t('link')}</button></form>
    {creating&&<form className="pr-form" onSubmit={e=>{e.preventDefault();void run('create',{title,body,head,base,draft,kind:itemKind}).then(v=>{if(v)setCreating(false);});}}>
      <label>{t('prTitle')}<input required value={title} onChange={e=>setTitle(e.target.value)}/></label>
      {itemKind==='pr'&&<><label>{t('head')}<input required value={head} onChange={e=>setHead(e.target.value)}/></label><label>{t('base')}<input required value={base} onChange={e=>setBase(e.target.value)}/></label></>}
      <label>{t('body')}<textarea value={body} onChange={e=>setBody(e.target.value)}/></label><label className="pr-checkbox"><input type="checkbox" checked={draft} onChange={e=>setDraft(e.target.checked)}/>{t('draft')}</label>
      <div className="pr-toolbar"><button type="button" disabled={busy} onClick={()=>{void run('generate',{head,base}).then(v=>{if(v){setTitle(string(v.title));setBody(string(v.body));}});}}>{t('generate')}</button><button className="pr-primary" disabled={busy}>{t('create')}</button></div>
    </form>}
    {!links.length&&<PublishRepository threadId={threadId} deviceId={deviceId}/>}
    {!links.length&&<p>{t('empty')}</p>}
    <ul>{links.map(link=><li key={link.url}><button className="pr-item" type="button" onClick={()=>{void open(link);}}>{link.repository.owner}/{link.repository.name} · {link.kind==='issue'?'Issue':'PR'} #{link.number}</button><a href={link.url} target="_blank" rel="noreferrer" aria-label={link.url}><ExternalLink size={14}/></a><button type="button" disabled={busy} aria-label={`${t('unlink')} #${link.number}`} onClick={()=>{void run('unlink',{url:link.url});}}><X size={14}/></button></li>)}</ul>
    {selected&&<div className="pr-form"><div className="pr-toolbar"><strong>#{selected.number} {detail&&string(detail.title)}</strong><button type="button" aria-label={t('close')} onClick={()=>{setSelected(null);setExtra(null);}}><X size={16}/></button></div>
      {!detail?<p>{t('loading')}</p>:<>
        <p>{t('state')}: {string(detail.state)} {watching&&` · ${t('watching')}`}</p>
        <label>{t('prTitle')}<input value={title} onChange={e=>setTitle(e.target.value)}/></label><label>{t('body')}<textarea value={body} onChange={e=>setBody(e.target.value)}/></label>
        <div className="pr-toolbar"><button disabled={busy} onClick={()=>{void run('update',{...itemInput(),title,body});}}>{t('update')}</button>
          {selected.kind==='pr'&&<><button disabled={busy} onClick={()=>{void run(watching?'unwatch':'watch',itemInput()).then(v=>{if(v)setWatching(!watching);});}}>{watching?t('unwatch'):t('watch')}</button><button disabled={busy} onClick={()=>{void run('ready',{...itemInput(),nodeId:detail.node_id,title});}}>{t('ready')}</button><button disabled={busy} onClick={()=>{void run('workOn',itemInput()).then(v=>{if(v)setExtra(v);});}}>{t('workOn')}</button></>}
        </div>
        <div className="pr-toolbar">{(['comments',...(selected.kind==='pr'?['checks','reviews','commits','files']:[])] as const).map(action=><button key={action} disabled={busy} onClick={()=>{void run(action,{...itemInput(),headSha:headOid(detail)}).then(v=>{if(v)setExtra(v.data);});}}>{t(action as 'checks')}</button>)}</div>
        <form onSubmit={e=>{e.preventDefault();void run('comment',{...itemInput(),body:comment}).then(v=>{if(v)setComment('');});}}><label>{t('comment')}<textarea required value={comment} onChange={e=>setComment(e.target.value)}/></label><button disabled={busy||!comment.trim()}>{t('comment')}</button></form>
        {selected.kind==='pr'&&<div className="pr-toolbar"><Select aria-label={t('mergeMethod')} value={method} onValueChange={setMethod} options={[{value:'squash',label:'Squash'},{value:'merge',label:'Merge'},{value:'rebase',label:'Rebase'}]}/><button className="pr-danger" disabled={busy||!headOid(detail)} onClick={()=>{void confirmAsync({title:t('merge'),description:t('mergeBody'),confirmLabel:t('merge'),tone:'danger'}).then(ok=>{if(ok)void run('merge',{...itemInput(),expectedHead:headOid(detail),method}).then(v=>{if(v)void open(selected);});});}}>{t('merge')}</button><button disabled={busy} onClick={()=>{void run('autoMerge',{...itemInput(),nodeId:detail.node_id,expectedHead:headOid(detail),method});}}>{t('autoMerge')}</button></div>}
      </>}
    </div>}
    {extra!==null&&<details open><summary>{t('details')}</summary><ResultDetails value={extra} onLink={link=>{void run('link',{url:link});}}/></details>}
  </section>;
}

function ResultDetails({value,onLink}:{value:unknown;onLink:(url:string)=>void}) {
  if (!value || typeof value !== 'object') return <p>{string(value)||t('result')}</p>;
  const data=value as Data;
  if(data.workingCopy) {const copy=data.workingCopy as Data;return <div><p>{t('result')}</p><PathChip path={string(copy.path)}/></div>;}
  if(data.authenticated!==undefined) return <p>{data.authenticated?t('signedIn'):t('signedOut')}</p>;
  const rows=Array.isArray(value)?value: data.check_runs||data.values||data.items||data.value||[value];
  if(!Array.isArray(rows))return <p>{t('result')}</p>;
  return <ul>{rows.map((row:Data,index:number)=>{
    const title=string(row.title)||string(row.name)||string(row.path)||string(row.filename)||string((row.commit as Data)?.message)||string(row.state)||string(row.status);
    const body=string(row.body)||string(row.description)||string((row.content as Data)?.raw)||string(row.message);
    const state=string(row.conclusion)||string(row.status)||string(row.state);
    const href=string(row.html_url)||string(row.web_url)||string(row.details_url)||string(row.target_url);
    return <li className="pr-result" key={index}><strong>{title}</strong>{state&&state!==title&&<span>{state}</span>}{body&&<p>{body}</p>}{/^https?:\/\//.test(href)&&<a href={href} target="_blank" rel="noreferrer">{t('details')}</a>}{/^https?:\/\//.test(href)&&<button onClick={()=>onLink(href)}>{t('link')}</button>}</li>;
  })}</ul>;
}
