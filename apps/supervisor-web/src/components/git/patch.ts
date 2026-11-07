/** A unified diff split into files and classified lines for display. The text is
    shown as received; parsing only decides colors and file boundaries. */
export type PatchLineKind = 'add' | 'del' | 'ctx' | 'hunk' | 'note';
export interface PatchLine { kind: PatchLineKind; text: string }
export interface PatchFile { path: string; oldPath?: string | undefined; additions: number; deletions: number; binary: boolean; created: boolean; removed: boolean; lines: PatchLine[] }
export interface ParsedPatch { preamble: PatchLine[]; files: PatchFile[] }

/** Git's quoted paths contain C escapes, including octal UTF-8 bytes. Decode
    only path metadata; the patch itself must remain exactly as received. */
function unquote(value: string) {
  if (!value.startsWith('"') || !value.endsWith('"')) return value;
  const escapes: Record<string, number> = { a: 7, b: 8, f: 12, n: 10, r: 13, t: 9, v: 11, '"': 34, '\\': 92 };
  const encoder = new TextEncoder();
  const bytes: number[] = [];
  for (const token of value.slice(1, -1).matchAll(/\\([0-7]{1,3}|[abfnrtv"\\])|[^\\]+|\\/gu)) {
    const escaped = token[1];
    if (escaped) bytes.push(/^[0-7]/.test(escaped) ? parseInt(escaped, 8) : escapes[escaped]!);
    else bytes.push(...encoder.encode(token[0]));
  }
  return new TextDecoder().decode(new Uint8Array(bytes));
}
const strip = (value: string) => unquote(value.trim()).replace(/^[ab]\//, '');

function headerPath(line: string) {
  // Retain the quotes until decoding, including for binary/rename-only patches
  // without +++ headers. Text patches can refine the destination from +++.
  const match = /^diff --git ("(?:\\.|[^"\\])*"|a\/.+?) ("(?:\\.|[^"\\])*"|b\/.+)$/.exec(line);
  return match ? { oldPath: strip(match[1]!), path: strip(match[2]!) } : null;
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
