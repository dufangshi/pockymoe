import { cp, mkdir, access, readFile, rm } from 'node:fs/promises';
import { createReadStream } from 'node:fs';
import { spawn } from 'node:child_process';
import { randomBytes, createHash } from 'node:crypto';
import net from 'node:net';
import { root, stateDir, treerSource, project, proxyPort, webPort, run, cleanEnv,
  requireDocker, api, save, read, until, deviceExec, sessionCookie, docker, prepareRuntimeConfig, processStartedAt } from './common.mjs';

async function prepare() {
  await mkdir(`${stateDir}/build/bin`, { recursive: true, mode: 0o700 });
  await run('cargo', ['build', '--locked', '-p', 'treer-proxy', '-p', 'treer-agent-server',
    '-p', 'treer-agent-host', '-p', 'treer-cli', '--target-dir', `${stateDir}/treer-target`],
  { cwd: treerSource, inherit: true, timeout: 900_000 });
  await run('cargo', ['build', '--locked', '-p', 'pockymoe'], { inherit: true, timeout: 900_000 });
  for (const binary of ['treer', 'treer-proxy', 'treer-agent-server', 'treer-agent-host']) {
    await cp(`${stateDir}/treer-target/debug/${binary}`, `${stateDir}/build/bin/${binary}`);
  }
  const binary = process.env.TREER_LAB_POCKYMOE_BIN ?? `${root}/target/debug/pockymoe`;
  await cp(binary, `${stateDir}/build/bin/pockymoe`);
  const web = process.env.TREER_LAB_POCKYMOE_WEB_DIST ?? `${root}/apps/supervisor-web/dist`;
  await access(`${web}/index.html`);
  await rm(`${stateDir}/build/web`, { recursive: true, force: true });
  await cp(web, `${stateDir}/build/web`, { recursive: true });
  await cp(`${root}/scripts/treer-lab`, `${stateDir}/build/lab`, { recursive: true });
  await run('corepack', ['pnpm', 'install', '--ignore-workspace', '--frozen-lockfile'],
    { cwd: `${treerSource}/web`, inherit: true, timeout: 300_000 });
  const binaryHash = createHash('sha256');
  for await (const chunk of createReadStream(binary)) binaryHash.update(chunk);
  await save('build.json', {
    pockymoeCheckoutCommit: (await run('git', ['rev-parse', 'HEAD'])).trim(),
    pockymoeBinarySource: binary, pockymoeBinarySha256: binaryHash.digest('hex'),
    treerCommit: (await run('git', ['rev-parse', 'HEAD'], { cwd: treerSource })).trim(),
    pockymoeWebDist: web, builtAt: new Date().toISOString(),
  });
  await save('compose.lab.json', {
    services: {
      // Reuse upstream Postgres configuration. Neither Postgres nor NATS is published.
      postgres: { restart: 'no' },
      proxy: {
        image: `${project}:device`,
        build: { context: `${stateDir}/build`, dockerfile: `${root}/scripts/treer-lab/host-device.Dockerfile` },
        restart: 'no', depends_on: { postgres: { condition: 'service_healthy' } },
        // Start Proxy later with ADMIN_PASSWORD supplied over stdin, never Docker config.
        entrypoint: ['/usr/bin/sleep'], command: ['infinity'],
        environment: { TREER_ENABLE_CORE_MESSAGES: 'true', TREER_NATS_URL: '',
          TREER_UPDATER_URL: '', TREER_UPDATER_TOKEN: '', DATABASE_URL: 'postgres://treer:local-test-only@postgres/treer' },
        ports: [{ target: 8787, published: String(proxyPort), host_ip: '127.0.0.1', protocol: 'tcp' }],
        networks: ['default', 'frontend'],
      },
      device: {
        image: `${project}:device`, init: true, restart: 'no',
        hostname: 'pockymoe-lab-device',
        depends_on: { proxy: { condition: 'service_started' } },
        volumes: ['device-workspace:/workspace', 'device-home:/home/lab'],
        environment: { TREER_NETWORK_MODE: 'plain' },
      },
    },
    // Docker suppresses published ports on internal-only networks. Only Proxy joins
    // this second private bridge; its sole published port still binds loopback.
    networks: { default: { internal: true }, frontend: { driver: 'bridge' } },
    volumes: { 'device-workspace': {}, 'device-home': {} },
  });
  await prepareRuntimeConfig();
  console.log('Prepared native binaries, isolated build context, web dependencies and Compose overlay.');
}

async function up() {
  if (!process.argv.includes('--skip-prepare')) await prepare();
  if (process.argv.includes('--prepare-only')) return;
  await requireDocker();
  await prepareRuntimeConfig();
  // Use the normalized minimal config after safety checks, avoiding upstream sidecars.
  await runRuntime(['up', '-d', '--build', 'postgres', 'proxy', 'device'], true);
  let existingSession = false;
  try { await sessionCookie(); await api('/api/auth/me'); existingSession = true; } catch { /* cold start */ }
  if (!existingSession) {
    const proxyId = (await runRuntime(['ps', '-q', 'proxy'])).trim();
    const adminPassword = randomBytes(32).toString('base64url');
    // Only restart our Proxy. No inherited process or production Supervisor is touched.
    await runRuntime(['restart', 'proxy']);
    await docker(['exec', '-i', proxyId, '/bin/sh', '-c',
      'IFS= read -r ADMIN_PASSWORD; export ADMIN_PASSWORD; nohup treer-proxy --listen 0.0.0.0:8787 --public-url "$1" --app-public-url "$2" >/tmp/treer-lab-proxy.log 2>&1 </dev/null &',
      'sh', `http://127.0.0.1:${proxyPort}/`, `http://127.0.0.1:${webPort}/`], { input: adminPassword + '\n' });
    await until('Proxy health', () => api('/api/health'));
    await api('/api/admin/login', { password: adminPassword });
  }
  await until('Proxy health', () => api('/api/health'));
  let state;
  try { state = await read(); } catch { /* new lab */ }
  if (!state) {
    await api('/api/admin/me');
    const invitation = await api('/api/admin/invitations', {});
    await api('/api/auth/register', { invite: invitation.token, email: `${project}@treer.invalid`,
      preferred_name: 'Local lab user', password: randomBytes(32).toString('base64url') });
    const user = await api('/api/auth/me');
    const { organization } = await api('/api/organizations', { name: 'Pockymoe local lab' });
    const { workspace } = await api('/api/workspaces', { organization_id: organization.organization_id,
      name: 'Treer integration lab' });
    state = { workspaceId: workspace.workspace_id, organizationId: organization.organization_id,
      userId: user.user?.user_id ?? user.user_id, project, proxyPort, webPort, authEnabled: true };
    await save('state.json', state);
  } else if (!existingSession) {
    const reset = await api(`/api/admin/users/${state.userId}/password-reset`, {});
    const password = randomBytes(32).toString('base64url');
    await api('/api/auth/reset-password', { token: new URL(reset.url).searchParams.get('reset'), password });
    await api('/api/auth/login', { email: `${project}@treer.invalid`, password });
  }
  if (!existingSession) {
    const child = spawn(process.execPath, [`${root}/scripts/treer-lab/session-broker.mjs`], {
      cwd: root, env: cleanEnv(), detached: true, stdio: ['pipe', 'ignore', 'ignore'],
    });
    child.stdin.end(JSON.stringify({ cookie: await sessionCookie() }));
    child.unref();
    await save('session-process.json', { pid: child.pid, startedAt: await processStartedAt(child.pid) });
    await until('in-memory session broker', async () => { await access(`${stateDir}/session.sock`); return true; });
  }
  const snapshot = await api(`/api/workspaces/${state.workspaceId}/snapshot`);
  if (!snapshot.servers?.some(server => server.status === 'online')) {
    const bootstrap = await api(`/api/workspaces/${state.workspaceId}/bootstrap`, {});
    // Enrollment key travels only via stdin -> shell variable -> process environment.
    await deviceExec(['/bin/sh', '-c',
      'IFS= read -r TREER_ENROLLMENT_KEY; export TREER_ENROLLMENT_KEY; exec treer-agent-server connect --proxy http://127.0.0.1:8787 --root /workspace --name lab-device --non-interactive --accept-risk --service-mode nohup'],
    { input: bootstrap.enrollment_key + '\n', timeout: 60_000 });
  }
  await until('enrolled device', async () => {
    const s = await api(`/api/workspaces/${state.workspaceId}/snapshot`);
    return s.servers?.some(server => server.status === 'online');
  });
  await startWeb();
  console.log(`Lab ready: Proxy 127.0.0.1:${proxyPort}; Web 127.0.0.1:${webPort}; device enrolled.`);
}

async function runRuntime(args, inherit = false) {
  const { docker } = await import('./common.mjs');
  return docker(['compose', '--project-name', project, '-f', `${stateDir}/compose.runtime.json`, ...args],
    { inherit, timeout: 600_000 });
}
async function refreshDevice() {
  if (!process.argv.includes('--skip-prepare')) await prepare();
  await requireDocker();
  const state = await read();
  // Proxy owns the shared image build; rebuilding it does not replace its container.
  await runRuntime(['build', 'proxy'], true);
  await runRuntime(['up', '-d', '--no-deps', 'device'], true);
  await deviceExec(['treer-agent-server', 'service', '--workspace', state.workspaceId, 'start']);
  await until('refreshed enrolled device', async () =>
    (await api(`/api/workspaces/${state.workspaceId}/snapshot`)).servers.some(server => server.status === 'online'));
  console.log('Refreshed only the lab device; Proxy, Postgres and sessions preserved.');
}
async function startWeb() {
  try {
    const { pid, startedAt } = await read('web-process.json');
    if (await processStartedAt(pid) === startedAt) { process.kill(pid, 0); return; }
  } catch { /* absent */ }
  await new Promise((resolve, reject) => {
    const socket = net.createServer();
    socket.once('error', () => reject(new Error(`Web port ${webPort} is occupied; refusing to reuse another service`)));
    socket.listen(webPort, '127.0.0.1', () => socket.close(resolve));
  });
  const child = spawn(process.execPath, [`${treerSource}/web/node_modules/vite/bin/vite.js`,
    '--host', '127.0.0.1', '--port', String(webPort), '--strictPort'], {
    cwd: `${treerSource}/web`, env: { ...cleanEnv(), TREER_DEV_PROXY: `http://127.0.0.1:${proxyPort}` },
    detached: true, stdio: 'ignore',
  });
  await new Promise((resolve, reject) => { child.once('spawn', resolve); child.once('error', reject); });
  child.unref();
  await save('web-process.json', { pid: child.pid, startedAt: await processStartedAt(child.pid),
    command: 'treer-lab-vite', root: treerSource });
  await until('Treer web', async () => (await fetch(`http://127.0.0.1:${webPort}/`)).ok);
}
async function down(reset = false) {
  try {
    const { pid, startedAt } = await read('session-process.json');
    const command = await readFile(`/proc/${pid}/cmdline`, 'utf8');
    if (await processStartedAt(pid) === startedAt && command.includes(`${root}/scripts/treer-lab/session-broker.mjs`)) process.kill(pid, 'SIGTERM');
    await rm(`${stateDir}/session-process.json`, { force: true });
  } catch { /* no owned broker */ }
  try {
    const { pid, root: source, startedAt } = await read('web-process.json');
    const command = await readFile(`/proc/${pid}/cmdline`, 'utf8');
    if (await processStartedAt(pid) === startedAt && command.includes(`${source}/web/node_modules/vite/bin/vite.js`)) process.kill(-pid, 'SIGTERM');
    await rm(`${stateDir}/web-process.json`, { force: true });
  } catch { /* no matching owned process */ }
  await requireDocker();
  await runRuntime(['down', ...(reset ? ['--volumes'] : [])], true);
  if (reset) await rm(`${stateDir}/state.json`, { force: true });
  console.log(reset ? 'Removed this lab containers and volumes; retained build artifacts/evidence.' : 'Stopped this lab; volumes retained.');
}

try {
  const command = process.argv[2];
  if (command === 'up') await up();
  else if (command === 'refresh-device') await refreshDevice();
  else if (command === 'down') await down();
  else if (command === 'reset' && process.argv.includes('--confirm')) await down(true);
  else throw new Error('Usage: up.sh [--prepare-only|--skip-prepare], lab.mjs refresh-device [--skip-prepare], down.sh, reset.sh --confirm (deletes only lab volumes)');
} catch (error) { console.error(error.message); process.exitCode = 1; }
