/** A unified diff split into files and classified lines for display. The text is
    shown as received; parsing only decides colors and file boundaries. `meta`
    lines (`index`, `new file mode`, `rename from`, `Binary files … differ`)
    restate what the file's heading already shows, so the view leaves them out;
    copying the patch still copies every line. */
export type PatchLineKind = 'add' | 'del' | 'ctx' | 'hunk' | 'note' | 'meta';
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

// Git's extended header lines that the file heading shows in words.
const META = /^(new file mode|deleted file mode|similarity index|dissimilarity index|rename from|rename to|copy from|copy to|Binary files |GIT binary patch)/;

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
  // Lines a hunk still holds on each side (from its `@@ -a,b +c,d @@`), so a
  // removed `-- x` followed by an added `++ y` stays content, and the hunk ends
  // where its counts do. Null outside a hunk.
  let hunk: { old: number; new: number } | null = null;
  // Whether the open file already had its `---`/`+++` pair: a binary untracked
  // file has no hunk, so the next untracked file's pair must still open a file.
  let headed = false;
  const lines = patch.split('\n');
  if (lines.at(-1) === '') lines.pop();
  for (let index = 0; index < lines.length; index += 1) {
    const text = lines[index]!;
    // A file header without `diff --git` (an untracked file's patch, or a plain
    // unified diff) still opens a file: its `---`/`+++` pair names it, and it
    // is never shown as a deleted and an added line.
    const open = !file || (hunk ? !Number.isFinite(hunk.old) : headed || file.binary);
    if (open && text.startsWith('--- ') && lines[index + 1]?.startsWith('+++ ')) {
      const from = strip(text.slice(4)), to = strip(lines[index + 1]!.slice(4));
      file = { path: to === '/dev/null' ? from : to, oldPath: from === '/dev/null' ? undefined : from, additions: 0, deletions: 0, binary: false, created: from === '/dev/null', removed: to === '/dev/null', lines: [] };
      files.push(file); hunk = null; headed = true; index += 1; continue;
    }
    // Hunk content always starts with a space, `+`, `-` or `\\`, so this is a header.
    if (text.startsWith('diff --git ')) {
      const names = headerPath(text);
      file = { path: names?.path ?? text.slice(11), oldPath: names?.oldPath, additions: 0, deletions: 0, binary: false, created: false, removed: false, lines: [] };
      files.push(file); hunk = null; headed = false; continue;
    }
    if (!file) { preamble.push({ kind: text.startsWith('+') ? 'add' : text.startsWith('-') ? 'del' : text.startsWith('@@') ? 'hunk' : 'ctx', text }); continue; }
    if (text.startsWith('@@')) {
      const counts = /^@@ -\d+(?:,(\d+))? \+\d+(?:,(\d+))? @@/.exec(text);
      hunk = counts ? { old: Number(counts[1] ?? 1), new: Number(counts[2] ?? 1) } : { old: Infinity, new: Infinity };
      file.lines.push({ kind: 'hunk', text });
      continue;
    }
    if (!hunk) {
      if (text.startsWith('+++ ')) { const next = strip(text.slice(4)); if (next === '/dev/null') file.removed = true; else file.path = next; headed = true; continue; }
      if (text.startsWith('--- ')) { if (strip(text.slice(4)) === '/dev/null') file.created = true; continue; }
      if (text.startsWith('index ')) continue;
      if (text.startsWith('Binary files') || text.startsWith('GIT binary patch')) file.binary = true;
      if (text.startsWith('new file mode')) file.created = true;
      if (text.startsWith('deleted file mode')) file.removed = true;
      file.lines.push({ kind: META.test(text) ? 'meta' : 'note', text });
      continue;
    }
    if (text.startsWith('\\')) { file.lines.push({ kind: 'note', text }); continue; }
    if (text.startsWith('+')) { file.additions += 1; hunk.new -= 1; file.lines.push({ kind: 'add', text }); }
    else if (text.startsWith('-')) { file.deletions += 1; hunk.old -= 1; file.lines.push({ kind: 'del', text }); }
    else { hunk.old -= 1; hunk.new -= 1; file.lines.push({ kind: 'ctx', text }); }
    if (hunk.old <= 0 && hunk.new <= 0) hunk = null;
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
