import type { ThreadDto } from '@pockymoe/shared';
import { say } from './sample';

/**
 * The sample device's terminal: a few everyday commands over the sample files,
 * so the tour can show typing, output and several terminals without a machine.
 */
export class TourShell {
  private line = '';
  private history = '';
  private cwd = '';
  private listeners = new Set<(data: string) => void>();

  constructor(
    private readonly root: string,
    private readonly files: () => Record<string, string>,
    private readonly threads: () => ThreadDto[],
  ) {
    this.history = `${say('这是教程的样例终端，输入 help 查看可用命令。', 'This is the tour\'s sample terminal; type help for its commands.')}\r\n${this.prompt()}`;
  }

  private prompt() {
    const name = this.root.split('/').pop() ?? '';
    return `\x1b[32myou@pockymoe-tour\x1b[0m:\x1b[34m~/${name}${this.cwd ? `/${this.cwd}` : ''}\x1b[0m$ `;
  }

  listen(send: (data: string) => void) {
    this.listeners.add(send);
    return () => this.listeners.delete(send);
  }

  replay() {
    return this.history;
  }

  clear() {
    this.history = this.prompt();
    this.write('\x1b[2J\x1b[H' + this.prompt(), false);
  }

  private write(data: string, keep = true) {
    if (keep) this.history = (this.history + data).slice(-20_000);
    for (const send of this.listeners) send(data);
  }

  input(data: string) {
    for (const char of data) {
      if (char === '\r' || char === '\n') {
        const command = this.line.trim();
        this.line = '';
        this.write('\r\n');
        const out = command ? this.run(command) : '';
        if (out === '\f') {
          this.clear();
          continue;
        }
        this.write(`${out ? `${out.replace(/\n/g, '\r\n')}\r\n` : ''}${this.prompt()}`);
      } else if (char === '\x7f' || char === '\b') {
        if (this.line) {
          this.line = [...this.line].slice(0, -1).join('');
          this.write('\b \b');
        }
      } else if (char === '\x03') {
        this.line = '';
        this.write(`^C\r\n${this.prompt()}`);
      } else if (char === '\x0c') {
        this.clear();
      } else if (char >= ' ' && char !== '\x1b') {
        this.line += char;
        this.write(char);
      }
    }
  }

  private resolve(path = '') {
    const parts = (path.startsWith('/') ? [] : this.cwd.split('/').filter(Boolean));
    for (const part of path.split('/')) {
      if (!part || part === '.') continue;
      if (part === '..') parts.pop();
      else parts.push(part);
    }
    return parts.join('/');
  }

  private entries(dir: string) {
    const prefix = dir ? `${dir}/` : '';
    const names = new Set<string>();
    for (const path of Object.keys(this.files())) {
      if (!path.startsWith(prefix)) continue;
      const [name, ...rest] = path.slice(prefix.length).split('/');
      if (name) names.add(rest.length ? `\x1b[34m${name}/\x1b[0m` : name);
    }
    return [...names].sort();
  }

  private run(command: string): string {
    const [name, ...args] = command.split(/\s+/);
    const files = this.files();
    switch (name) {
      case 'help':
        return say(
          '可用命令：ls, cd, cat, pwd, echo, clear, git status, npm test, pockymoe thread list',
          'Commands: ls, cd, cat, pwd, echo, clear, git status, npm test, pockymoe thread list',
        );
      case 'ls': {
        const dir = this.resolve(args.find((arg) => !arg.startsWith('-')) ?? '');
        const entries = this.entries(dir);
        return entries.length ? entries.join('  ') : files[dir] !== undefined ? dir : `ls: ${dir}: No such file or directory`;
      }
      case 'cd': {
        const dir = this.resolve(args[0] ?? '/');
        if (dir && !this.entries(dir).length) return `cd: ${args[0]}: No such directory`;
        this.cwd = dir;
        return '';
      }
      case 'cat':
        return args.map((arg) => files[this.resolve(arg)]?.replace(/\n$/, '') ?? `cat: ${arg}: No such file`).join('\n');
      case 'pwd':
        return `${this.root}${this.cwd ? `/${this.cwd}` : ''}`;
      case 'echo':
        return args.join(' ');
      case 'clear':
        return '\f';
      case 'git':
        return args[0] === 'status' ? 'On branch main\nnothing to commit, working tree clean' : `git ${args.join(' ')}: ${say('样例终端只支持 git status', 'the sample terminal only knows git status')}`;
      case 'npm':
        return args[0] === 'test' ? ' ✓ tests/login.test.ts (3 tests) 12ms\n\n Test Files  1 passed (1)\n      Tests  3 passed (3)' : `npm ${args.join(' ')}: ${say('样例终端只支持 npm test', 'the sample terminal only knows npm test')}`;
      case 'pockymoe':
        if (args[0] === 'thread' && args[1] === 'list') {
          return this.threads().map((thread) => `${thread.id.slice(0, 8)}  ${thread.parentThreadId ? '  └ ' : ''}${thread.title}  (${thread.status})`).join('\n');
        }
        return say('样例终端只支持 pockymoe thread list；Agent 在你的设备上用完整的 pockymoe CLI 互相发消息。', 'The sample terminal only knows pockymoe thread list; on your devices agents use the full pockymoe CLI to message each other.');
      default:
        return `${name}: ${say('样例终端里没有这个命令（输入 help 查看）', 'not in the sample terminal (type help)')}`;
    }
  }
}
