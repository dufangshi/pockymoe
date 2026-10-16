/* Completed turns fold to their answer unless the reader turns that off in
   Settings; then summaries carry the agent's narration (`narration=1`) and the
   transcript keeps it between folded runs of work. */

export const AUTO_COLLAPSE_COMPLETED_TURNS_STORAGE_KEY =
  'remote-codex-auto-collapse-completed-turns';

/** Fired on `window` when the preference changes, so open threads reload their summary. */
export const TRANSCRIPT_PREFERENCE_EVENT = 'pockymoe:transcript-preference';

export function readAutoCollapseCompletedTurns() {
  try {
    return window.localStorage.getItem(AUTO_COLLAPSE_COMPLETED_TURNS_STORAGE_KEY) !== 'false';
  } catch {
    return true;
  }
}

/** Query parameter for summary requests: narration only when turns stay open. */
export function narrationQuery() {
  return readAutoCollapseCompletedTurns() ? '' : '&narration=1';
}
