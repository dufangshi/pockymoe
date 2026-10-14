import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, readFile, writeFile, chmod } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import http from 'node:http';

export const root = path.resolve(fileURLToPath(new URL('../..', import.meta.url)));
export const stateDir = path.join(root, '.local/treer-lab');
export const treerSource = process.env.TREER_LAB_SOURCE ?? '/home/ubuntu/dev/remoteCodex/reference/treer';
export const project = `pockymoe-lab-${createHash('sha256').update(root).digest('hex').slice(0, 10)}`;
export const proxyPort = Number(process.env.TREER_LAB_PROXY_PORT ?? 18787);
export const webPort = Number(process.env.TREER_LAB_WEB_PORT ?? 15173);
for (const port of [proxyPort, webPort]) {
  if (!Number.isInteger(port) || port < 1024 || port > 65535) throw new Error('Invalid lab port');
}
export const origin = `http://127.0.0.1:${proxyPort}`;
const cookies = new Map();
export async function sessionCookie() {
  if (cookies.size) return [...cookies].map(([key, value]) => `${key}=${value}`).join('; ');
  return new Promise((resolve, reject) => {
    const request = http.get({ socketPath: `${stateDir}/session.sock`, path: '/session' }, response => {
      let body = ''; response.on('data', data => { body += data; });
      response.on('end', () => response.statusCode === 200 ? resolve(JSON.parse(body).cookie) : reject(new Error('Lab session unavailable')));
    });
    request.on('error', reject);
    request.setTimeout(2000, () => request.destroy(new Error('Lab session timed out')));
  });
}
export const quote = value => `'${String(value).replaceAll("'", "'\\''")}'`;

// Do not inherit the active Pockymoe thread, relay, harness or Treer identity.
export function cleanEnv() {
  return Object.fromEntries(Object.entries(process.env).filter(([key]) =>
    ['PATH', 'HOME', 'USER', 'LANG', 'TERM', 'TMPDIR', 'XDG_CACHE_HOME', 'RUSTUP_HOME', 'CARGO_HOME'].includes(key)));
}
export function run(command, args, { input, cwd = root, inherit = false, env = {}, timeout = 120_000 } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd, env: { ...cleanEnv(), ...env },
      stdio: ['pipe', inherit ? 'inherit' : 'pipe', inherit ? 'inherit' : 'pipe'] });
    let stdout = '';
    child.stdout?.on('data', chunk => { stdout += chunk; });
    // Captured stderr can contain credentials from upstream failures; never print it.
    child.stderr?.resume();
    const timer = setTimeout(() => child.kill('SIGKILL'), timeout);
    child.on('error', error => { clearTimeout(timer); reject(error); });
    child.on('close', code => {
      clearTimeout(timer);
      if (code === 0) resolve(stdout);
      else reject(new Error(`${path.basename(command)} failed (exit ${code}); inspect private lab state locally`));
    });
    child.stdin.end(input);
  });
}
export async function docker(args, options) {
  // Socket ACLs allow existing sessions to work without re-login or newgrp.
  // Set TREER_LAB_USE_SG=1 on hosts that grant only supplementary group access.
  return process.env.TREER_LAB_USE_SG === '1'
    ? run('sg', ['docker', '-c', ['docker', ...args].map(quote).join(' ')], options)
    : run('docker', args, options);
}
export async function requireDocker() {
  try { await docker(['info', '--format', '{{.ServerVersion}}']); }
  catch { throw new Error('Docker unavailable. Check socket ACL/getent group docker; use TREER_LAB_USE_SG=1 if sg is installed and group membership is new.'); }
}
export const compose = (args, options) => docker(['compose', '--project-name', project,
  '--project-directory', treerSource, '-f', `${treerSource}/compose.yaml`,
  '-f', `${stateDir}/compose.lab.json`, ...args], {
  ...options, env: { POSTGRES_PASSWORD: 'local-test-only', ADMIN_PASSWORD: 'unused-local-test',
    DATABASE_URL: 'postgres://treer:local-test-only@postgres/treer',
    TREER_UPDATER_TOKEN: 'unused-local-test', ...options?.env },
});
export async function prepareRuntimeConfig() {
  // `compose config` is read-only and does not need the Docker daemon/group.
  const output = await run('docker', ['compose', '--project-name', project,
    '--project-directory', treerSource, '-f', `${treerSource}/compose.yaml`,
    '-f', `${stateDir}/compose.lab.json`, 'config', '--format', 'json'], { env: {
    POSTGRES_PASSWORD: 'local-test-only', ADMIN_PASSWORD: 'unused-local-test',
    DATABASE_URL: 'postgres://treer:local-test-only@postgres/treer', TREER_UPDATER_TOKEN: 'unused-local-test',
  } });
  const config = JSON.parse(output);
  // Compose merges upstream wildcard ports/dependencies; discard them before use.
  config.services.proxy.ports = [{ target: 8787, published: String(proxyPort), host_ip: '127.0.0.1', protocol: 'tcp' }];
  config.services.proxy.depends_on = { postgres: { condition: 'service_healthy', required: true } };
  config.services.proxy.environment = {
    DATABASE_URL: 'postgres://treer:local-test-only@postgres/treer', TREER_ENABLE_CORE_MESSAGES: 'true',
  };
  delete config.services.app;
  delete config.services.nats;
  delete config.services.updater;
  for (const service of Object.values(config.services)) {
    if (service.ports?.some(port => port.host_ip !== '127.0.0.1')) throw new Error('Non-loopback published lab port');
  }
  if (!config.networks.default.internal) throw new Error('Lab Docker network must be private');
  await save('compose.runtime.json', config);
  return config;
}
export async function save(name, value) {
  await mkdir(stateDir, { recursive: true, mode: 0o700 });
  await chmod(stateDir, 0o700);
  await writeFile(path.join(stateDir, name), JSON.stringify(value, null, 2) + '\n', { mode: 0o600 });
}
export async function read(name = 'state.json') {
  return JSON.parse(await readFile(path.join(stateDir, name), 'utf8'));
}
export async function processStartedAt(pid) {
  const stat = await readFile(`/proc/${pid}/stat`, 'utf8');
  return stat.slice(stat.lastIndexOf(')') + 2).split(' ')[19];
}
export async function api(route, body) {
  const cookie = await sessionCookie().catch(() => '');
  const response = await fetch(origin + route, {
    method: body === undefined ? 'GET' : 'POST',
    headers: { 'content-type': 'application/json', ...(cookie ? { cookie } : {}) },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(20_000),
  });
  for (const header of response.headers.getSetCookie()) {
    const [pair] = header.split(';');
    const index = pair.indexOf('=');
    cookies.set(pair.slice(0, index), pair.slice(index + 1));
  }
  if (!response.ok) throw new Error(`Treer API ${route.split('?')[0]}: HTTP ${response.status}`);
  return response.json();
}
export async function until(label, check, timeout = 45_000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    try { const result = await check(); if (result) return result; } catch { /* startup race */ }
    await new Promise(resolve => setTimeout(resolve, 250));
  }
  throw new Error(`Timed out: ${label}`);
}
export async function deviceExec(args, options) {
  const id = (await compose(['ps', '-q', 'device'])).trim();
  if (!id) throw new Error('Lab device is not running');
  return docker(['exec', '-i', id, ...args], options);
}
