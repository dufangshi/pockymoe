import { getLocale } from '@pockymoe/thread-ui/i18n';

// Web-owned copy for the new-thread form's worktree option (docs/worktrees.md).
const messages = {
  toggle: ['新建 worktree', 'New worktree'],
  hint: ['在旁边的独立检出里工作，有自己的分支', 'Works in its own checkout beside the repository, on its own branch'],
  branch: ['分支', 'Branch'],
  base: ['起点', 'Base'],
  baseHint: ['新分支从这里开始', 'Where a new branch starts'],
  setupFailed: ['worktree 已建好，但仓库的 setup 失败', 'The worktree is ready but the repository setup failed'],
  setupRan: ['worktree setup 完成', 'Worktree setup finished'],
  openThread: ['打开线程', 'Open thread'],
} as const;
export type WorktreeMessage = keyof typeof messages;
export function worktreeText(key: WorktreeMessage): string {
  return messages[key][getLocale() === 'zh-CN' ? 0 : 1];
}

/** The worktree's name the device derives from a title (`name_from_title`), for the preview. */
export function worktreeNameFromTitle(title: string): string | null {
  let slug = '';
  for (const c of title.trim().toLowerCase()) {
    if (/[a-z0-9]/.test(c)) slug += c;
    else if (slug && !slug.endsWith('-')) slug += '-';
  }
  slug = slug.slice(0, 32).replace(/-+$/, '');
  return /^[a-z][a-z0-9_-]{0,31}$/.test(slug) ? slug : null;
}

/** `/x/repo` → `/x/repo.worktrees/NAME`: where the device puts the worktree. */
export function worktreePathPreview(workspacePath: string, name: string): string {
  const trimmed = workspacePath.replace(/\/+$/, '');
  return `${trimmed}.worktrees/${name}`;
}
