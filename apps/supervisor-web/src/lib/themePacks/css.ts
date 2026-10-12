/* Theme pack stylesheet compiler. It parses each stylesheet into rules, rejects
   anything that could reach the network or run code, and rewrites the pack-local
   syntax:

   - `:theme` -> `:root[data-theme-preset="<id>"]` (every top-level selector must
     start from it, so a pack only styles the page while it is applied);
   - `url("asset:<path>")` -> the URL the caller resolves for that pack file;
   - `@keyframes` names and `@font-face` families -> prefixed with `<id>--`, and
     the references to them in `animation`, `font-family`, `font` and custom
     properties.

   Only `asset:` and `data:image/*` / `data:font/*` URLs are allowed. */

import { CssSyntaxError, lineAt, tokenizeCss, type CssToken } from './cssTokenizer';
import { packError } from './errors';
import { THEME_PACK_LIMITS, isFontPath, isImagePath, normalizePackPath } from './limits';

type Node =
  | { kind: 'at'; name: string; prelude: CssToken[]; block: Node[] | null; offset: number }
  | { kind: 'rule'; prelude: CssToken[]; block: Node[]; offset: number; context: RuleContext }
  | { kind: 'decl'; name: string; value: CssToken[]; offset: number };

type RuleContext = 'top' | 'nested' | 'keyframe';
type BlockType = 'rules' | 'decls' | 'keyframes' | 'descriptors';

const TOP_AT_RULES = new Set(['media', 'supports', 'container', 'keyframes', '-webkit-keyframes', 'font-face']);
const NESTED_AT_RULES = new Set(['media', 'supports', 'container']);

export interface CompileOptions {
  id: string;
  /** Families declared by the manifest's `fonts`; @font-face families are added. */
  fontFamilies?: string[];
  /** URL for a pack file referenced with `asset:`; throws when it is missing. */
  resolveAsset: (path: string, usage: 'image' | 'font') => string;
}

interface Sheet {
  file: string;
  source: string;
  nodes: Node[];
}

class Compiler {
  readonly keyframes = new Set<string>();
  readonly families = new Map<string, string>();
  private file = '';
  private source = '';

  constructor(private readonly options: CompileOptions) {
    for (const family of options.fontFamilies ?? []) this.families.set(family.toLowerCase(), family);
  }

  fail(offset: number, message: string): never {
    packError('css', `${this.file}:${lineAt(this.source, offset)}: ${message}`);
  }

  parse(file: string, raw: string): Sheet {
    const source = raw.replace(/\r\n?|\f/g, '\n');
    this.file = file;
    this.source = source;
    let tokens: CssToken[];
    try {
      tokens = tokenizeCss(source);
    } catch (error) {
      if (error instanceof CssSyntaxError) this.fail(error.offset, error.message);
      throw error;
    }
    const offsets: number[] = [];
    let position = 0;
    for (const token of tokens) {
      offsets.push(position);
      position += token.raw.length;
    }
    const parser = new Parser(tokens, offsets, (offset, message) => this.fail(offset, message));
    const nodes = parser.parseBlock(0, tokens.length, 'rules', 0, 'top');
    return { file, source, nodes };
  }

  collect(sheet: Sheet) {
    this.file = sheet.file;
    this.source = sheet.source;
    const walk = (nodes: Node[]) => {
      for (const node of nodes) {
        if (node.kind === 'at') {
          const name = node.name.toLowerCase();
          if (name === 'keyframes' || name === '-webkit-keyframes') {
            const keyframe = significant(node.prelude);
            if (keyframe.length !== 1 || (keyframe[0]!.type !== 'ident' && keyframe[0]!.type !== 'string'))
              this.fail(node.offset, '@keyframes needs one name');
            this.keyframes.add(tokenValue(keyframe[0]!));
          } else if (name === 'font-face') {
            const family = node.block?.find((child) => child.kind === 'decl' && child.name.toLowerCase() === 'font-family');
            if (!family || family.kind !== 'decl') this.fail(node.offset, '@font-face needs a font-family');
            const value = familyName(family.value);
            if (!value) this.fail(family.offset, 'font-family must be one name');
            this.families.set(value.toLowerCase(), value);
          }
          if (node.block) walk(node.block);
        } else if (node.kind === 'rule') walk(node.block);
      }
    };
    walk(sheet.nodes);
  }

  emit(sheet: Sheet) {
    this.file = sheet.file;
    this.source = sheet.source;
    return this.emitNodes(sheet.nodes, false, '');
  }

  private emitNodes(nodes: Node[], inFontFace: boolean, indent: string): string {
    return nodes.map((node) => this.emitNode(node, inFontFace, indent)).join('\n');
  }

  private emitNode(node: Node, inFontFace: boolean, indent: string): string {
    if (node.kind === 'decl') return `${indent}${this.emitDeclaration(node, inFontFace)}`;
    if (node.kind === 'rule') {
      const prelude = node.context === 'top' ? this.emitSelector(node.prelude, node.offset) : this.emitPlain(node.prelude, 'selector', node.offset);
      return `${indent}${prelude.trim()} {\n${this.emitNodes(node.block, false, `${indent}  `)}\n${indent}}`;
    }
    const name = node.name.toLowerCase();
    if (name === 'keyframes' || name === '-webkit-keyframes') {
      const keyframe = significant(node.prelude)[0]!;
      return `${indent}@${name} ${this.prefixedName(tokenValue(keyframe), keyframe.type === 'string')} {\n${this.emitNodes(node.block ?? [], false, `${indent}  `)}\n${indent}}`;
    }
    const prelude = this.emitPlain(node.prelude, 'prelude', node.offset).trim();
    if (name === 'font-face') return `${indent}@font-face {\n${this.emitNodes(node.block ?? [], true, `${indent}  `)}\n${indent}}`;
    return `${indent}@${name} ${prelude} {\n${this.emitNodes(node.block ?? [], false, `${indent}  `)}\n${indent}}`;
  }

  private emitSelector(prelude: CssToken[], offset: number) {
    for (const selector of splitTopLevel(prelude, ',')) {
      const parts = trimTokens(selector);
      if (!firstCompoundHasTheme(parts)) this.fail(offset, 'each selector must start from :theme');
    }
    return this.emitPlain(prelude, 'selector', offset);
  }

  /** Tokens outside declarations: selectors and at-rule preludes. */
  private emitPlain(tokens: CssToken[], where: 'selector' | 'prelude', offset: number): string {
    let out = '';
    for (let index = 0; index < tokens.length; index += 1) {
      const token = tokens[index]!;
      this.checkToken(token, where, offset);
      if (
        token.type === ':' &&
        tokens[index - 1]?.type !== ':' &&
        tokens[index + 1]?.type === 'ident' &&
        (tokens[index + 1] as { value: string }).value.toLowerCase() === 'theme'
      ) {
        out += `:root[data-theme-preset="${this.options.id}"]`;
        index += 1;
        continue;
      }
      if (token.type === 'url' || (token.type === 'function' && isUrlFunction(token.name)))
        this.fail(offset, `url() is not allowed in a ${where}`);
      out += token.type === 'comment' ? '/**/' : token.raw;
    }
    return out;
  }

  private emitDeclaration(node: Extract<Node, { kind: 'decl' }>, inFontFace: boolean): string {
    const name = node.name.toLowerCase();
    const bare = name.replace(/^-[a-z]+-/, '');
    if (bare === 'behavior' || bare === 'binding') this.fail(node.offset, `${node.name} is not allowed`);
    const custom = name.startsWith('--');
    let value = node.value;
    if (inFontFace && name === 'font-family') {
      const family = familyName(value);
      if (!family) this.fail(node.offset, 'font-family must be one name');
      return `font-family: ${quote(`${this.options.id}--${family}`)};`;
    }
    const familyProperty = name === 'font-family' || name === 'font' || custom;
    const animationProperty = name === 'animation' || name === 'animation-name' || custom;
    if (familyProperty && this.families.size) value = this.rewriteFamilies(value);
    const text = this.emitValue(value, animationProperty, inFontFace ? 'font' : 'image', node.offset);
    return `${custom ? node.name : name}:${text};`;
  }

  /** Font family references, as strings or unquoted ident runs at the end of a
   *  comma-separated segment. */
  private rewriteFamilies(tokens: CssToken[]): CssToken[] {
    const out: CssToken[] = [];
    for (const [index, segment] of splitTopLevel(tokens, ',').entries()) {
      if (index > 0) out.push({ type: ',', raw: ',' });
      let end = segment.length;
      while (end > 0 && segment[end - 1]!.type === 'ws') end -= 1;
      let start = end;
      while (start > 0 && (segment[start - 1]!.type === 'ident' || (segment[start - 1]!.type === 'ws' && start - 1 > 0 && segment[start - 2]!.type === 'ident')))
        start -= 1;
      const run = segment.slice(start, end);
      const runName = run.filter((token) => token.type === 'ident').map((token) => (token as { value: string }).value).join(' ');
      const family = run.length && this.families.get(runName.toLowerCase());
      const replaced = segment.map((token) =>
        token.type === 'string' && this.families.has(token.value.toLowerCase())
          ? ({ type: 'string', raw: quote(`${this.options.id}--${this.families.get(token.value.toLowerCase())}`), value: '' } as CssToken)
          : token,
      );
      if (family) out.push(...replaced.slice(0, start), { type: 'string', raw: quote(`${this.options.id}--${family}`), value: '' }, ...replaced.slice(end));
      else out.push(...replaced);
    }
    return out;
  }

  private emitValue(tokens: CssToken[], animation: boolean, usage: 'image' | 'font', offset: number): string {
    let out = '';
    let depth = 0;
    const functions: string[] = [];
    for (let index = 0; index < tokens.length; index += 1) {
      const token = tokens[index]!;
      this.checkToken(token, 'value', offset);
      if (token.type === 'url') {
        out += `url(${quote(this.resolveUrl(token.value, usage, offset))})`;
        continue;
      }
      if (token.type === 'function') {
        const name = token.name.toLowerCase();
        if (isUrlFunction(name)) {
          // url("…") or src("…"): exactly one string argument.
          const close = matchingClose(tokens, index);
          const args = significant(tokens.slice(index + 1, close));
          if (args.length !== 1 || args[0]!.type !== 'string') this.fail(offset, `${name}() needs one quoted URL`);
          out += `url(${quote(this.resolveUrl(args[0]!.value, usage, offset))})`;
          index = close;
          continue;
        }
        functions.push(name);
        depth += 1;
        out += token.raw;
        continue;
      }
      if (token.type === '(') {
        functions.push('');
        depth += 1;
      } else if (token.type === ')') {
        functions.pop();
        depth -= 1;
      }
      if (token.type === 'string' && functions.some((name) => /(?:^|-)image-set$|^cross-fade$|^image$/.test(name))) {
        out += quote(this.resolveUrl(token.value, 'image', offset));
        continue;
      }
      if (animation && depth === 0 && (token.type === 'ident' || token.type === 'string') && this.keyframes.has(tokenValue(token))) {
        out += this.prefixedName(tokenValue(token), token.type === 'string');
        continue;
      }
      out += token.type === 'comment' ? '/**/' : token.raw;
    }
    return out;
  }

  private checkToken(token: CssToken, where: string, offset = 0) {
    const values =
      token.type === 'string' || token.type === 'url' || token.type === 'ident' ? [token.value]
      : token.type === 'function' ? [token.name]
      : [];
    for (const value of values) {
      const compact = value.toLowerCase().replace(/[\s\u0000-\u001f]/g, '');
      if (compact.includes('javascript:') || compact.includes('vbscript:')) this.fail(offset, `script URL in a ${where}`);
    }
    if (token.type === 'function') {
      const name = token.name.toLowerCase().replace(/^-[a-z]+-/, '');
      if (name === 'expression') this.fail(offset, 'expression() is not allowed');
      if (token.raw.includes('\\')) this.fail(offset, 'escaped function names are not allowed');
    }
    if (token.type === 'at' || token.type === 'cdo' || token.type === 'cdc') this.fail(offset, `${token.raw} is not allowed here`);
  }

  private resolveUrl(raw: string, usage: 'image' | 'font', offset: number): string {
    const value = raw.trim();
    const lower = value.toLowerCase();
    if (lower.startsWith('asset:')) {
      let path: string;
      try {
        path = normalizePackPath(value.slice('asset:'.length), 'asset');
      } catch {
        this.fail(offset, `${value} is not a pack path`);
      }
      if (!(usage === 'font' ? isFontPath(path) : isImagePath(path) || isFontPath(path)))
        this.fail(offset, `${value} must be ${usage === 'font' ? 'a font' : 'an image or font'}`);
      return this.options.resolveAsset(path, usage);
    }
    if (lower.startsWith('data:')) {
      if (!/^data:(?:image\/(?:png|jpeg|gif|webp|avif|svg\+xml)|font\/(?:woff2?|ttf|otf))[;,]/.test(lower))
        this.fail(offset, 'data: URLs must be images or fonts');
      return value;
    }
    this.fail(offset, `url(${value.slice(0, 60)}) must use asset: or data:`);
  }

  private prefixedName(name: string, asString: boolean) {
    const prefixed = `${this.options.id}--${name}`;
    return asString || !/^-?[a-zA-Z_][a-zA-Z0-9_-]*$/.test(name) ? quote(prefixed) : prefixed;
  }
}

class Parser {
  constructor(
    private readonly tokens: CssToken[],
    private readonly offsets: number[],
    private readonly fail: (offset: number, message: string) => never,
  ) {}

  private offset(index: number) {
    return this.offsets[index] ?? this.offsets[this.offsets.length - 1] ?? 0;
  }

  parseBlock(start: number, end: number, type: BlockType, depth: number, context: RuleContext): Node[] {
    if (depth > THEME_PACK_LIMITS.cssDepth) this.fail(this.offset(start), 'blocks are nested too deeply');
    return type === 'decls' || type === 'descriptors'
      ? this.parseDeclarations(start, end, depth, type === 'decls')
      : this.parseRules(start, end, depth, type === 'keyframes' ? 'keyframe' : context);
  }

  private parseRules(start: number, end: number, depth: number, context: RuleContext): Node[] {
    const nodes: Node[] = [];
    let index = start;
    while (index < end) {
      const token = this.tokens[index]!;
      if (token.type === 'ws' || token.type === 'comment' || token.type === 'cdo' || token.type === 'cdc') {
        index += 1;
        continue;
      }
      if (token.type === 'at') {
        index = this.parseAtRule(index, end, depth, nodes, context === 'keyframe' ? 'keyframe' : 'top');
        continue;
      }
      const open = this.scanTo(index, end, ['{']);
      if (open >= end || this.tokens[open]!.type !== '{') this.fail(this.offset(index), 'expected a block');
      const close = this.matching(open, end);
      nodes.push({
        kind: 'rule',
        prelude: this.tokens.slice(index, open),
        block: this.parseBlock(open + 1, close, 'decls', depth + 1, 'nested'),
        offset: this.offset(index),
        context,
      });
      index = close + 1;
    }
    return nodes;
  }

  private parseAtRule(index: number, end: number, depth: number, nodes: Node[], context: 'top' | 'keyframe' | 'nested'): number {
    const token = this.tokens[index] as Extract<CssToken, { type: 'at' }>;
    const name = token.name.toLowerCase();
    const offset = this.offset(index);
    if (token.raw.includes('\\')) this.fail(offset, 'escaped at-rule names are not allowed');
    const stop = this.scanTo(index + 1, end, ['{', ';']);
    if (name === 'charset' && context === 'top') return stop + 1;
    if (name === 'import' || name === 'namespace') this.fail(offset, `@${name} is not allowed`);
    const allowed = context === 'nested' ? NESTED_AT_RULES : context === 'top' ? TOP_AT_RULES : new Set<string>();
    if (!allowed.has(name)) this.fail(offset, `@${name} is not allowed${context === 'nested' ? ' inside a rule' : ''}`);
    if (stop >= end || this.tokens[stop]!.type !== '{') this.fail(offset, `@${name} needs a block`);
    const close = this.matching(stop, end);
    const blockType: BlockType =
      name === 'font-face' ? 'descriptors'
      : name.endsWith('keyframes') ? 'keyframes'
      : context === 'nested' ? 'decls'
      : 'rules';
    nodes.push({ kind: 'at', name, prelude: this.tokens.slice(index + 1, stop), block: this.parseBlock(stop + 1, close, blockType, depth + 1, 'top'), offset });
    return close + 1;
  }

  private parseDeclarations(start: number, end: number, depth: number, nested: boolean): Node[] {
    const nodes: Node[] = [];
    let index = start;
    while (index < end) {
      const token = this.tokens[index]!;
      if (token.type === 'ws' || token.type === 'comment' || token.type === ';') {
        index += 1;
        continue;
      }
      if (token.type === 'at') {
        if (!nested) this.fail(this.offset(index), `${token.raw} is not allowed here`);
        index = this.parseAtRule(index, end, depth, nodes, 'nested');
        continue;
      }
      let colon = index + 1;
      while (colon < end && (this.tokens[colon]!.type === 'ws' || this.tokens[colon]!.type === 'comment')) colon += 1;
      const isCustom = token.type === 'ident' && token.value.startsWith('--');
      const stop = this.scanTo(index, end, isCustom ? [';'] : [';', '{']);
      if (token.type === 'ident' && this.tokens[colon]?.type === ':' && (isCustom || stop >= end || this.tokens[stop]!.type === ';')) {
        if (token.raw.includes('\\')) this.fail(this.offset(index), 'escaped property names are not allowed');
        nodes.push({ kind: 'decl', name: token.value, value: this.tokens.slice(colon + 1, stop), offset: this.offset(index) });
        index = stop + 1;
        continue;
      }
      if (!nested || stop >= end || this.tokens[stop]!.type !== '{') this.fail(this.offset(index), 'invalid declaration');
      const close = this.matching(stop, end);
      nodes.push({
        kind: 'rule',
        prelude: this.tokens.slice(index, stop),
        block: this.parseBlock(stop + 1, close, 'decls', depth + 1, 'nested'),
        offset: this.offset(index),
        context: 'nested',
      });
      index = close + 1;
    }
    return nodes;
  }

  /** The first of `stops` at nesting depth 0, or `end`. */
  private scanTo(index: number, end: number, stops: string[]): number {
    let depth = 0;
    for (let at = index; at < end; at += 1) {
      const type = this.tokens[at]!.type;
      if (depth === 0 && stops.includes(type)) return at;
      if (type === '(' || type === '[' || type === 'function' || (type === '{' && !stops.includes('{'))) depth += 1;
      else if (type === ')' || type === ']' || type === '}') {
        if (depth === 0) {
          if (type === '}') return at;
          this.fail(this.offset(at), `unexpected ${type}`);
        }
        depth -= 1;
      }
    }
    return end;
  }

  private matching(open: number, end: number): number {
    const stack: string[] = [];
    for (let at = open; at < end; at += 1) {
      const type = this.tokens[at]!.type;
      if (type === '{') stack.push('}');
      else if (type === '(' || type === 'function') stack.push(')');
      else if (type === '[') stack.push(']');
      else if (type === '}' || type === ')' || type === ']') {
        if (stack.pop() !== type) this.fail(this.offset(at), `unexpected ${type}`);
        if (!stack.length) return at;
      }
    }
    this.fail(this.offset(open), 'unclosed block');
  }
}

function significant(tokens: CssToken[]) {
  return tokens.filter((token) => token.type !== 'ws' && token.type !== 'comment');
}
function trimTokens(tokens: CssToken[]) {
  let start = 0;
  let end = tokens.length;
  while (start < end && (tokens[start]!.type === 'ws' || tokens[start]!.type === 'comment')) start += 1;
  while (end > start && (tokens[end - 1]!.type === 'ws' || tokens[end - 1]!.type === 'comment')) end -= 1;
  return tokens.slice(start, end);
}
function tokenValue(token: CssToken) {
  return token.type === 'string' || token.type === 'ident' ? token.value : token.raw;
}
function isUrlFunction(name: string) {
  const lower = name.toLowerCase();
  return lower === 'url' || lower === 'src';
}
function matchingClose(tokens: CssToken[], open: number) {
  let depth = 0;
  for (let index = open; index < tokens.length; index += 1) {
    const type = tokens[index]!.type;
    if (type === 'function' || type === '(') depth += 1;
    else if (type === ')') {
      depth -= 1;
      if (depth === 0) return index;
    }
  }
  return tokens.length;
}
function splitTopLevel(tokens: CssToken[], separator: ',' | ';') {
  const parts: CssToken[][] = [[]];
  let depth = 0;
  for (const token of tokens) {
    if (token.type === '(' || token.type === '[' || token.type === 'function') depth += 1;
    else if (token.type === ')' || token.type === ']') depth -= 1;
    if (depth === 0 && token.type === separator) parts.push([]);
    else parts[parts.length - 1]!.push(token);
  }
  return parts;
}
function firstCompoundHasTheme(tokens: CssToken[]) {
  let depth = 0;
  for (let index = 0; index < tokens.length; index += 1) {
    const token = tokens[index]!;
    if (token.type === '(' || token.type === '[' || token.type === 'function') depth += 1;
    else if (token.type === ')' || token.type === ']') depth -= 1;
    if (depth === 0 && (token.type === 'ws' || (token.type === 'delim' && '>+~'.includes(token.raw)))) return false;
    if (
      depth === 0 &&
      token.type === ':' &&
      tokens[index - 1]?.type !== ':' &&
      tokens[index + 1]?.type === 'ident' &&
      (tokens[index + 1] as { value: string }).value.toLowerCase() === 'theme'
    )
      return true;
  }
  return false;
}
/** A single family name: one string, or idents separated by spaces. */
function familyName(tokens: CssToken[]) {
  const parts = significant(tokens);
  if (parts.length === 1 && parts[0]!.type === 'string') return parts[0]!.value.trim() || null;
  if (parts.length && parts.every((token) => token.type === 'ident')) return parts.map((token) => (token as { value: string }).value).join(' ');
  return null;
}
/** A double-quoted CSS string. */
export function quote(value: string) {
  return `"${value.replace(/[\\"]/g, (char) => `\\${char}`).replace(/[\n\r\f]/g, (char) => `\\${char.charCodeAt(0).toString(16)} `)}"`;
}

/** Compiles a pack's stylesheets (in order) into one stylesheet. */
export function compileStylesheets(sheets: { file: string; source: string }[], options: CompileOptions): string {
  const compiler = new Compiler(options);
  const parsed = sheets.map((sheet) => compiler.parse(sheet.file, sheet.source));
  for (const sheet of parsed) compiler.collect(sheet);
  return parsed.map((sheet) => `/* ${sheet.file} */\n${compiler.emit(sheet)}`).join('\n\n');
}
