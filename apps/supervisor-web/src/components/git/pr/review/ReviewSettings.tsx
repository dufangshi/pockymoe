import { useEffect, useState } from 'react';
import { Select } from '@pockymoe/thread-ui';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import type { WorkspaceDto } from '@pockymoe/shared';
import { request } from '../../../../lib/api';
import { rt } from './reviewText';

export interface ReviewSettingsValue {
  removeAgentCredits: boolean;
  guideInstructions: string;
  guideWorkspaceInstructions: Record<string, string>;
  guideSource: 'auto' | 'codex' | 'claude' | 'upstream';
  guideModel: string;
}

/** Settings → Source control → Pull request review: the merge default and the
    review guide's writer and instructions (a workspace can override them). */
export function ReviewSettings({ apiRoot, value, busy, save }: { apiRoot: string; value: ReviewSettingsValue | undefined; busy: boolean; save(input: Record<string, unknown>): Promise<void> }) {
  useI18n();
  const current: ReviewSettingsValue = value ?? { removeAgentCredits: false, guideInstructions: '', guideWorkspaceInstructions: {}, guideSource: 'auto', guideModel: '' };
  const [workspaces, setWorkspaces] = useState<WorkspaceDto[]>([]);
  const [scope, setScope] = useState('');
  const stored = scope ? current.guideWorkspaceInstructions[scope] ?? '' : current.guideInstructions;
  const [text, setText] = useState(stored);
  const [model, setModel] = useState(current.guideModel);
  useEffect(() => { setText(stored); }, [stored]);
  useEffect(() => { setModel(current.guideModel); }, [current.guideModel]);
  useEffect(() => { void request<WorkspaceDto[]>(`${apiRoot}/workspaces`).then(setWorkspaces).catch(() => {}); }, [apiRoot]);
  const review = (patch: Partial<ReviewSettingsValue>) => save({ review: patch });
  return <section className="pr-form" aria-label={rt('settingsTitle')}>
    <strong>{rt('settingsTitle')}</strong>
    <label className="pr-checkbox"><input type="checkbox" checked={current.removeAgentCredits} disabled={busy} onChange={event => { void review({ removeAgentCredits: event.target.checked }); }} />{rt('removeCreditsDefault')}</label>
    <p className="settings-hint">{rt('removeCreditsHint')}</p>
    <label>{rt('guideSource')}<Select value={current.guideSource} onValueChange={source => { void review({ guideSource: source as ReviewSettingsValue['guideSource'] }); }}
      options={[{ value: 'auto', label: rt('sourceAuto') }, { value: 'codex', label: 'Codex' }, { value: 'claude', label: 'Claude Code' }, { value: 'upstream', label: rt('sourceUpstream') }]} /></label>
    <form className="pr-form" onSubmit={event => { event.preventDefault(); void review({ guideModel: model.trim() }); }}>
      <label>{rt('guideModel')}<input value={model} spellCheck={false} autoCapitalize="off" onChange={event => setModel(event.target.value)} /></label>
      <button className="pr-primary" disabled={busy || model.trim() === current.guideModel}>{rt('save')}</button>
    </form>
    <form className="pr-form" onSubmit={event => {
      event.preventDefault();
      void (scope ? review({ guideWorkspaceInstructions: { ...current.guideWorkspaceInstructions, [scope]: text } }) : review({ guideInstructions: text }));
    }}>
      {workspaces.length > 0 && <label>{rt('guideWorkspace')}<Select value={scope} onValueChange={setScope} options={[{ value: '', label: rt('all') }, ...workspaces.map(workspace => ({ value: workspace.id, label: workspace.label }))]} /></label>}
      <label>{rt('guideInstructions')}<textarea maxLength={4000} value={text} onChange={event => setText(event.target.value)} /></label>
      <p className="settings-hint">{rt('guideInstructionsHint')}</p>
      <button className="pr-primary" disabled={busy || text === stored}>{rt('save')}</button>
    </form>
  </section>;
}
