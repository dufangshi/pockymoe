import { useEffect, useState } from 'react';
import { ErrorMark, Select } from '@pockymoe/thread-ui';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import type { ModelOptionDto, ThreadDto } from '@pockymoe/shared';
import { deviceRequest, interruptThread } from '../lib/api';

type Preview = { text: string; turns: number; estimatedTokens: number; resumesSession: boolean };
export function HarnessSwitchFields({ thread, deviceId, onSwitched }: { thread: ThreadDto; deviceId?: string; onSwitched: (fork?: ThreadDto) => void }) {
  const { locale } = useI18n();
  const zh = locale === 'zh-CN';
  const [mode, setMode] = useState('switch');
  const [turns, setTurns] = useState<{turnId:string;turnIndex:number}[]>([]);
  const [turnId, setTurnId] = useState('');
  const [agents, setAgents] = useState<ModelOptionDto[]>([]);
  const [agent, setAgent] = useState('');
  const [models, setModels] = useState<ModelOptionDto[]>([]);
  const [model, setModel] = useState('');
  const [preview, setPreview] = useState<Preview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const base = `api/threads/${encodeURIComponent(thread.id)}/harness`;
  useEffect(() => {
    let active = true;
    deviceRequest<{turnId:string;turnIndex:number}[]>(deviceId, `api/threads/${encodeURIComponent(thread.id)}/fork-turns`).then(value=>{if(active)setTurns(value);}).catch(()=>{});
    deviceRequest<ModelOptionDto[]>(deviceId, 'api/agent-runtimes/acp/agents').then(value => { if (active) setAgents(value); }).catch(e => { if (active) setError(String(e)); });
    return () => { active = false; };
  }, [deviceId,thread.id]);
  useEffect(() => {
    let active = true;
    setModels([]); setModel(''); setPreview(null);
    if (agent) deviceRequest<ModelOptionDto[]>(deviceId, `api/agent-runtimes/acp/models?agentId=${encodeURIComponent(agent)}`).then(value => {
      if (active) { setModels(value); setModel(value.find(m => m.isDefault)?.model ?? value[0]?.model ?? ''); }
    }).catch(e => { if (active) setError(String(e)); });
    return () => { active = false; };
  }, [agent, deviceId]);
  const payload = { provider: 'acp', agentId: agent, model: model || null, reasoningEffort: null };
  const body = mode === 'fork' ? {target:payload,turnId:turnId || null} : payload;
  async function act(switching: boolean) {
    setBusy(true); setError(null);
    try {
      if (!switching) setPreview(await deviceRequest<Preview>(deviceId, `${base}/preview`, { method: 'POST', body: JSON.stringify(body) }));
      else {
        if (thread.status === 'running' && mode === 'switch') {
          await interruptThread(thread.id, {}, deviceId);
          // Release waits for the interrupted turn to settle before changing its harness.
          await deviceRequest(deviceId, `api/threads/${encodeURIComponent(thread.id)}/release`, {method:'POST',body:JSON.stringify({interrupt:true})});
        }
        const result=await deviceRequest<ThreadDto>(deviceId, `${base}/${mode}`, { method: 'POST', body: JSON.stringify(body) });
        onSwitched(mode==='fork' ? result : undefined);
      }
    } catch (e) { setError(String(e)); } finally { setBusy(false); }
  }
  return <section className="space-y-3 border-t border-[var(--theme-border)] pt-4">
    <label className="block">{zh ? '操作' : 'Action'}<Select className="host-input mt-1 w-full rounded-md border p-2" value={mode} disabled={busy} onValueChange={v=>{setMode(v);setPreview(null);}} aria-label={zh ? '交接操作' : 'Handoff action'}><option value="switch">{zh ? '切换 Harness' : 'Switch harness'}</option><option value="fork">{zh ? '使用其他 Harness 分叉' : 'Fork to another harness'}</option></Select></label>
    {mode==='fork' && <label className="block">{zh ? '保留至' : 'Keep through'}<Select className="host-input mt-1 w-full rounded-md border p-2" value={turnId} disabled={busy} onValueChange={v=>{setTurnId(v);setPreview(null);}} aria-label={zh ? '分叉轮次' : 'Fork turn'}><option value="">{zh ? '最新一轮' : 'Latest turn'}</option>{turns.map(t=><option key={t.turnId} value={t.turnId}>{zh ? '轮次' : 'Turn'} {t.turnIndex}</option>)}</Select></label>}
    <label className="block">{zh ? '切换 Harness' : 'Switch harness'}
      <Select className="host-input mt-1 w-full rounded-md border p-2" value={agent} disabled={busy} onValueChange={setAgent} aria-label={zh ? '目标 Harness' : 'Target harness'}>
        <option value="">{zh ? '选择 Harness' : 'Choose a harness'}</option>
        {agents.filter(a => a.id !== thread.agentId).map(a => <option key={a.id} value={a.id}>{a.displayName}</option>)}
      </Select>
    </label>
    {agent && <label className="block">{zh ? '模型' : 'Model'}<Select className="host-input mt-1 w-full rounded-md border p-2" value={model} disabled={busy} onValueChange={v => { setModel(v); setPreview(null); }} aria-label={zh ? '目标模型' : 'Target model'}>
      {models.map(m => <option key={m.id} value={m.model}>{m.displayName}</option>)}
    </Select></label>}
    {preview && <div className="space-y-2">
      <p>{zh ? `交接 ${preview.turns} 轮 · 约 ${preview.estimatedTokens} tokens` : `${preview.turns} ${preview.turns===1 ? 'turn' : 'turns'} · ≈${preview.estimatedTokens} tokens`}{preview.resumesSession ? (zh ? ' · 恢复已有会话' : ' · Resume saved session') : ''}</p>
      <p className="settings-note">{zh ? '计划、沙箱、快速模式及推理强度将重置；审批模式保留。切换前请清除目标。' : 'Plan, sandbox, fast mode and reasoning effort reset. Approval mode stays. Clear any goal first.'}</p>
      <details><summary>{zh ? '交接内容' : 'History preview'}</summary><pre className="max-h-64 overflow-auto whitespace-pre-wrap break-words text-xs">{preview.text}</pre></details>
    </div>}
    {error && <p role="alert"><ErrorMark message={error} />{error}</p>}
    <div className="form-dialog-actions">
      {preview && <button type="button" disabled={busy} onClick={() => setPreview(null)}>{zh ? '取消' : 'Cancel'}</button>}
      <button type="button" className="relay-button-primary rounded-md px-3 py-2" disabled={!agent || !model || busy} onClick={() => void act(Boolean(preview))}>
        {preview ? (mode==='fork' ? (zh ? '分叉' : 'Fork') : thread.status === 'running' ? (zh ? '停止并切换' : 'Stop and switch') : (zh ? '切换' : 'Switch')) : (zh ? '预览交接' : 'Preview handoff')}
      </button>
    </div>
  </section>;
}
