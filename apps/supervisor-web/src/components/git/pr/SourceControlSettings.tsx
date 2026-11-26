import { CloneRepository } from './RepositoryActions';
import { ReviewSettings } from './review/ReviewSettings';
import { useEffect, useState } from 'react';
import { useLocation } from 'react-router-dom';
import { RefreshCw } from 'lucide-react';
import { Select, ErrorMark } from '@pockymoe/thread-ui';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { request, relayModeActive } from '../../../lib/api';
import { relayDeviceIdFromPath } from '../../../lib/relayRoutes';
import { sourceRoot, type SourceSettings } from './api';
import { prText as t } from './text';
import './pr.css';
export function SourceControlSettings() {
  useI18n();const {pathname}=useLocation();const deviceId=relayDeviceIdFromPath(pathname);
  if(relayModeActive()&&!deviceId)return <p>{t('chooseDevice')}</p>;
  return <Settings key={deviceId} apiRoot={sourceRoot(deviceId)}/>;
}
function Settings({apiRoot}:{apiRoot:string}) {
  const [settings,setSettings]=useState<SourceSettings>({hosts:[],writingInstructions:''});const [instructions,setInstructions]=useState('');const [host,setHost]=useState('github.com');const [kind,setKind]=useState('github');const [token,setToken]=useState('');const [error,setError]=useState('');const [saved,setSaved]=useState(false);const [busy,setBusy]=useState(false);
  const load=()=>request<SourceSettings>(`${apiRoot}/source-control/scan`,{method:'POST',body:'{}'}).then(v=>{setSettings(v);setInstructions(v.writingInstructions);});
  useEffect(()=>{let alive=true;void request<SourceSettings>(`${apiRoot}/source-control/settings`).then(v=>{if(alive){setSettings(v);setInstructions(v.writingInstructions);}}).catch(e=>{if(alive)setError(String(e));});return()=>{alive=false;};},[apiRoot]);
  const save=async(input:Record<string,unknown>)=>{setBusy(true);setError('');setSaved(false);try{const next=await request<SourceSettings>(`${apiRoot}/source-control/settings`,{method:'PUT',body:JSON.stringify(input)});setSettings(next);setToken('');setSaved(true);}catch(e){setError(String(e));}finally{setBusy(false);}};
  return <section className="pr-panel" aria-label={t('sourceControl')}>
    <div className="pr-toolbar"><strong>{t('sourceControl')}</strong><button type="button" aria-label={t('refresh')} disabled={busy} onClick={()=>{void load().catch(e=>setError(String(e)));}}><RefreshCw size={16}/></button></div>
    {error&&<ErrorMark message={error}/>}{saved&&<p role="status">{t('saved')}</p>}
    <ul>{settings.hosts.map(h=><li key={h.host}><span>{h.host} · {h.authenticated===undefined?(h.hasToken?t('configured'):t('cli')):(h.authenticated?t('signedIn'):t('signedOut'))}</span><label className="pr-checkbox"><input type="checkbox" checked={h.enabled} disabled={busy} onChange={e=>{void save({host:h.host,kind:h.kind,enabled:e.target.checked});}}/>{t('enabled')}</label></li>)}</ul>
    <form className="pr-form" onSubmit={e=>{e.preventDefault();void save({host,kind,...(token?{token}:{})});}}>
      <label>{t('host')}<input required value={host} onChange={e=>setHost(e.target.value)}/></label>
      <label>{t('kind')}<Select value={kind} onValueChange={setKind} options={[{value:'github',label:'GitHub'},{value:'gitlab',label:'GitLab'},{value:'forgejo',label:'Forgejo / Gitea'},{value:'bitbucket',label:'Bitbucket'},{value:'azure',label:'Azure DevOps'}]}/></label>
      <label>{t('token')}<input type="password" autoComplete="new-password" value={token} onChange={e=>setToken(e.target.value)}/></label><p className="settings-note">{t('secretHint')}</p><button className="pr-primary" disabled={busy||!host.trim()}>{t('save')}</button>
    </form>
    <CloneRepository apiRoot={apiRoot}/>
    <label>{t('generationSource')}<Select value={settings.generation?.source||'auto'} onValueChange={source=>{void save({generation:{...settings.generation,source}});}} options={[{value:'auto',label:t('automatic')},{value:'codex',label:'Codex'},{value:'claude',label:'Claude'}]}/></label>
    <form className="pr-form" onSubmit={e=>{e.preventDefault();void save({writingInstructions:instructions});}}><label>{t('instructions')}<textarea value={instructions} onChange={e=>setInstructions(e.target.value)}/></label><p className="settings-note">{t('instructionsHint')}</p><button className="pr-primary" disabled={busy||instructions===settings.writingInstructions}>{t('save')}</button></form>
    {settings.review&&<ReviewSettings apiRoot={apiRoot} value={settings.review} busy={busy} save={save}/>}
  </section>;
}
