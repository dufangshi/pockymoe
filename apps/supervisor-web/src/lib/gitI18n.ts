import { getLocale } from '@pockymoe/thread-ui/i18n';

// Web-owned additions to the shared `workbench.git*` copy, following the shared locale.
const messages = {
  copyHead: ['复制 HEAD 提交 ID', 'Copy HEAD commit ID'],
  viewObserved: ['查看该副本', 'View that copy'],
  noFiles: ['此范围没有更改的文件。', 'No changed files in this scope.'],
  base: ['基准', 'Base'],
  apply: ['比较', 'Compare'],
  untracked: ['未跟踪', 'Untracked'],
  added: ['新增', 'Added'],
  modified: ['修改', 'Modified'],
  deleted: ['删除', 'Deleted'],
  renamed: ['重命名', 'Renamed'],
  copied: ['复制', 'Copied'],
  conflicted: ['冲突', 'Conflicted'],
  typeChanged: ['类型变更', 'Type changed'],
  changed: ['已更改', 'Changed'],
  binary: ['二进制文件', 'Binary file'],
  page: ['第 {n} 页', 'Page {n}'],
  backToFiles: ['返回文件列表', 'Back to files'],
  readOnlyHint: ['此面板只读：不包含 PR 状态或 Git 写操作。', 'This panel is read only: no PR status or Git writes.'],
} as const;
export type GitMessage = keyof typeof messages;
export function gitText(key: GitMessage, vars?: Record<string, string | number>) {
  const text: string = messages[key][getLocale() === 'zh-CN' ? 0 : 1];
  return vars ? text.replace(/\{(\w+)\}/g, (match, name: string) => name in vars ? String(vars[name]) : match) : text;
}
