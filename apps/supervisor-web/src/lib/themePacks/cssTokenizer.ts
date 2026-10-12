/* A CSS tokenizer after CSS Syntax Level 3. The theme pack compiler checks the
   decoded values (escapes resolved), so `u\72l(` and `\40 import` are caught the
   same way the browser would read them. */

export type CssToken =
  | { type: 'ws'; raw: string }
  | { type: 'comment'; raw: string }
  | { type: 'string'; raw: string; value: string }
  | { type: 'url'; raw: string; value: string }
  | { type: 'function'; raw: string; name: string }
  | { type: 'ident'; raw: string; value: string }
  | { type: 'at'; raw: string; name: string }
  | { type: 'hash'; raw: string; value: string }
  | { type: 'number'; raw: string }
  | { type: 'delim'; raw: string }
  | { type: '(' | ')' | '[' | ']' | '{' | '}' | ':' | ';' | ','; raw: string }
  | { type: 'cdo' | 'cdc'; raw: string };

export class CssSyntaxError extends Error {
  constructor(
    message: string,
    readonly offset: number,
  ) {
    super(message);
  }
}

const isDigit = (c: string | undefined) => c !== undefined && c >= '0' && c <= '9';
const isHex = (c: string | undefined) => c !== undefined && /^[0-9a-fA-F]$/.test(c);
const isNameStart = (c: string | undefined) => c !== undefined && (/^[a-zA-Z_]$/.test(c) || c.charCodeAt(0) >= 0x80);
const isName = (c: string | undefined) => c !== undefined && (isNameStart(c) || isDigit(c) || c === '-');
const isWs = (c: string | undefined) => c === ' ' || c === '\t' || c === '\n' || c === '\r' || c === '\f';
const isNonPrintable = (c: string | undefined) => c !== undefined && /^[\u0000-\u0008\u000b\u000e-\u001f\u007f]$/.test(c);

export function tokenizeCss(input: string): CssToken[] {
  // Preprocessing as in the spec: newlines normalized, NUL replaced.
  const source = input.replace(/\r\n?|\f/g, '\n').replace(/\u0000/g, '�');
  const tokens: CssToken[] = [];
  let i = 0;
  const peek = (offset = 0) => source[i + offset];
  const validEscape = (offset = 0) => peek(offset) === '\\' && peek(offset + 1) !== '\n' && peek(offset + 1) !== undefined;
  const startsIdent = (offset = 0) => {
    const c = peek(offset);
    if (c === '-') return isNameStart(peek(offset + 1)) || peek(offset + 1) === '-' || validEscape(offset + 1);
    if (isNameStart(c)) return true;
    return validEscape(offset);
  };
  const startsNumber = (offset = 0) => {
    const c = peek(offset);
    if (c === '+' || c === '-') return isDigit(peek(offset + 1)) || (peek(offset + 1) === '.' && isDigit(peek(offset + 2)));
    if (c === '.') return isDigit(peek(offset + 1));
    return isDigit(c);
  };
  // Consumes the escape after a backslash at `i` and returns the code point.
  const consumeEscape = () => {
    i += 1; // backslash
    if (isHex(peek())) {
      let hex = '';
      while (hex.length < 6 && isHex(peek())) hex += source[i++];
      if (isWs(peek())) i += 1;
      const code = parseInt(hex, 16);
      return code === 0 || code > 0x10ffff || (code >= 0xd800 && code <= 0xdfff) ? '�' : String.fromCodePoint(code);
    }
    if (peek() === undefined) return '�';
    const char = String.fromCodePoint(source.codePointAt(i)!);
    i += char.length;
    return char;
  };
  const consumeName = () => {
    let value = '';
    for (;;) {
      if (isName(peek())) value += source[i++];
      else if (validEscape()) value += consumeEscape();
      else return value;
    }
  };
  const consumeNumberText = () => {
    if (peek() === '+' || peek() === '-') i += 1;
    while (isDigit(peek())) i += 1;
    if (peek() === '.' && isDigit(peek(1))) {
      i += 1;
      while (isDigit(peek())) i += 1;
    }
    if ((peek() === 'e' || peek() === 'E') && (isDigit(peek(1)) || ((peek(1) === '+' || peek(1) === '-') && isDigit(peek(2))))) {
      i += 2;
      while (isDigit(peek())) i += 1;
    }
  };
  const consumeString = (quote: string, start: number): CssToken => {
    i += 1;
    let value = '';
    for (;;) {
      const c = peek();
      if (c === undefined) break; // EOF closes the string, as in browsers
      if (c === quote) {
        i += 1;
        break;
      }
      if (c === '\n') throw new CssSyntaxError('unterminated string', start);
      if (c === '\\') {
        if (peek(1) === undefined) {
          i += 1;
          continue;
        }
        if (peek(1) === '\n') {
          i += 2;
          continue;
        }
        value += consumeEscape();
        continue;
      }
      value += c;
      i += 1;
    }
    return { type: 'string', raw: source.slice(start, i), value };
  };
  const consumeUrl = (start: number): CssToken => {
    // `url(` already consumed; whitespace may follow.
    while (isWs(peek())) i += 1;
    let value = '';
    for (;;) {
      const c = peek();
      if (c === undefined || c === ')') {
        i += c === ')' ? 1 : 0;
        return { type: 'url', raw: source.slice(start, i), value };
      }
      if (isWs(c)) {
        while (isWs(peek())) i += 1;
        if (peek() === ')' || peek() === undefined) continue;
        throw new CssSyntaxError('bad url', start);
      }
      if (c === '"' || c === "'" || c === '(' || isNonPrintable(c)) throw new CssSyntaxError('bad url', start);
      if (c === '\\') {
        if (!validEscape()) throw new CssSyntaxError('bad url', start);
        value += consumeEscape();
        continue;
      }
      value += c;
      i += 1;
    }
  };
  const consumeIdentLike = (start: number): CssToken => {
    const name = consumeName();
    if (peek() === '(') {
      i += 1;
      if (name.toLowerCase() === 'url') {
        let j = i;
        while (isWs(source[j])) j += 1;
        if (source[j] !== '"' && source[j] !== "'") return consumeUrl(start);
      }
      return { type: 'function', raw: source.slice(start, i), name };
    }
    return { type: 'ident', raw: source.slice(start, i), value: name };
  };

  while (i < source.length) {
    const start = i;
    const c = source[i]!;
    if (c === '/' && peek(1) === '*') {
      const end = source.indexOf('*/', i + 2);
      i = end < 0 ? source.length : end + 2;
      tokens.push({ type: 'comment', raw: source.slice(start, i) });
    } else if (isWs(c)) {
      while (isWs(peek())) i += 1;
      tokens.push({ type: 'ws', raw: source.slice(start, i) });
    } else if (c === '"' || c === "'") {
      tokens.push(consumeString(c, start));
    } else if (c === '#') {
      if (isName(peek(1)) || validEscape(1)) {
        i += 1;
        tokens.push({ type: 'hash', raw: '', value: consumeName() });
        tokens[tokens.length - 1]!.raw = source.slice(start, i);
      } else {
        i += 1;
        tokens.push({ type: 'delim', raw: '#' });
      }
    } else if ('()[]{}:;,'.includes(c)) {
      i += 1;
      tokens.push({ type: c as '(', raw: c });
    } else if (startsNumber()) {
      consumeNumberText();
      if (startsIdent()) consumeName();
      else if (peek() === '%') i += 1;
      tokens.push({ type: 'number', raw: source.slice(start, i) });
    } else if (c === '-' && peek(1) === '-' && peek(2) === '>') {
      i += 3;
      tokens.push({ type: 'cdc', raw: '-->' });
    } else if (c === '<' && source.startsWith('<!--', i)) {
      i += 4;
      tokens.push({ type: 'cdo', raw: '<!--' });
    } else if (startsIdent()) {
      tokens.push(consumeIdentLike(start));
    } else if (c === '@') {
      if (startsIdent(1)) {
        i += 1;
        const name = consumeName();
        tokens.push({ type: 'at', raw: source.slice(start, i), name });
      } else {
        i += 1;
        tokens.push({ type: 'delim', raw: '@' });
      }
    } else if (c === '\\') {
      throw new CssSyntaxError('stray backslash', start);
    } else {
      const char = String.fromCodePoint(source.codePointAt(i)!);
      i += char.length;
      tokens.push({ type: 'delim', raw: char });
    }
  }
  return tokens;
}

/** Line number of an offset, for error messages. */
export function lineAt(source: string, offset: number) {
  let line = 1;
  for (let index = 0; index < offset && index < source.length; index += 1) if (source[index] === '\n') line += 1;
  return line;
}
