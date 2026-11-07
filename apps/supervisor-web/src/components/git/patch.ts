/** A unified diff split into files and classified lines for display. The text is
    shown as received; parsing only decides colors and file boundaries. */
export type PatchLineKind = 'add' | 'del' | 'ctx' | 'hunk' | 'note';
export interface PatchLine { kind: PatchLineKind; text: string }
export interface PatchFile { path: string; oldPath?: string | undefined; additions: number; deletions: number; binary: boolean; created: boolean; removed: boolean; lines: PatchLine[] }
export interface ParsedPatch { preamble: PatchLine[]; files: PatchFile[] }

const unquote = (value: string) => value.startsWith('"') && value.endsWith('"') ? value.slice(1, -1) : value;
const strip = (value: string) => unquote(value.trim()).replace(/^[ab]\//, '');

function headerPath(line: string) {
  // `diff --git a/x b/x`: take the b side; quoted or spaced names fall back to +++.
  const match = /^diff --git (?:"?a\/(.+?)"?) (?:"?b\/(.+?)"?)$/.exec(line);
  return match ? { oldPath: match[1]!, path: match[2]! } : null;
}

export function parsePatch(patch: string): ParsedPatch {
  const preamble: PatchLine[] = [];
  const files: PatchFile[] = [];
  let file: PatchFile | null = null;
  let inHunk = false;
  const lines = patch.split('\n');
  if (lines.at(-1) === '') lines.pop();
  for (const text of lines) {
    if (text.startsWith('diff --git ')) {
      const names = headerPath(text);
      file = { path: names?.path ?? text.slice(11), oldPath: names?.oldPath, additions: 0, deletions: 0, binary: false, created: false, removed: false, lines: [] };
      files.push(file); inHunk = false; continue;
    }
    if (!file) { preamble.push({ kind: text.startsWith('+') ? 'add' : text.startsWith('-') ? 'del' : text.startsWith('@@') ? 'hunk' : 'ctx', text }); continue; }
    if (text.startsWith('@@')) { inHunk = true; file.lines.push({ kind: 'hunk', text }); continue; }
    if (!inHunk) {
      if (text.startsWith('+++ ')) { const next = strip(text.slice(4)); if (next !== '/dev/null') file.path = next; continue; }
      if (text.startsWith('--- ') || text.startsWith('index ')) continue;
      if (text.startsWith('Binary files') || text.startsWith('GIT binary patch')) file.binary = true;
      if (text.startsWith('new file mode')) file.created = true;
      if (text.startsWith('deleted file mode')) file.removed = true;
      file.lines.push({ kind: 'note', text });
      continue;
    }
    if (text.startsWith('+')) { file.additions += 1; file.lines.push({ kind: 'add', text }); }
    else if (text.startsWith('-')) { file.deletions += 1; file.lines.push({ kind: 'del', text }); }
    else if (text.startsWith('\\')) file.lines.push({ kind: 'note', text });
    else file.lines.push({ kind: 'ctx', text });
  }
  for (const entry of files) if (entry.oldPath === entry.path) delete entry.oldPath;
  return { preamble, files };
}

export type FileChange = 'untracked' | 'added' | 'modified' | 'deleted' | 'renamed' | 'copied' | 'conflicted' | 'typeChanged' | 'changed';
/** What one porcelain status column means. */
export function fileChange(code: string, other = ' '): FileChange {
  if (code === 'U' || other === 'U' || (code === 'A' && other === 'A') || (code === 'D' && other === 'D')) return 'conflicted';
  switch (code) {
    case '?': return 'untracked';
    case 'A': return 'added';
    case 'M': return 'modified';
    case 'D': return 'deleted';
    case 'R': return 'renamed';
    case 'C': return 'copied';
    case 'T': return 'typeChanged';
    default: return 'changed';
  }
}
