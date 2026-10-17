// Spawned only by the opt-in fake runtime. Identity stays in process memory.
import { spawnSync } from 'node:child_process';
import { mkdir, writeFile, readdir, readFile, rename, open, rm } from 'node:fs/promises';

const thread = process.env.POCKYMOE_THREAD_ID;
if (!thread || !process.env.POCKYMOE_TOKEN) process.exit(2);
const dir = `${process.env.POCKYMOE_E2E_FAKE_CLI_JOB_DIR ?? '/workspace/l2-thread-jobs'}/${thread}`;
const binaries = {
  pockymoe: process.env.POCKYMOE_E2E_FAKE_CLI_BIN ?? '/usr/local/bin/pockymoe',
  treer: '/usr/local/bin/treer',
  // Treer App calls through the thread's inherited workload proxy (issues-e2e.mjs).
  curl: '/usr/bin/curl',
};
await mkdir(dir, { recursive: true, mode: 0o700 });
try {
  const lock = await open(`${dir}/lock`, 'wx', 0o600);
  await lock.close();
} catch { process.exit(0); }

const parent = process.ppid;
async function cleanup() { await rm(`${dir}/lock`, { force: true }); }
for (const signal of ['SIGTERM', 'SIGINT']) process.on(signal, async () => {
  await cleanup(); process.exit(0);
});
await writeFile(`${dir}/ready.json`, JSON.stringify({
  threadId: thread, agentId: process.env.TREER_AGENT_ID ?? null, pid: process.pid,
}), { mode: 0o600 });
try {
  for (;;) {
    if (process.ppid !== parent) break;
    for (const file of await readdir(dir)) {
      if (!/^[a-f0-9-]+\.request\.json$/.test(file)) continue;
      // Publish requests by writing a temporary file and atomically renaming it.
      const { cli = 'pockymoe', args } = JSON.parse(await readFile(`${dir}/${file}`, 'utf8'));
      if (!Object.hasOwn(binaries, cli)) throw new Error('Unsupported fixture CLI');
      const result = spawnSync(binaries[cli], args, {
        env: { ...process.env, TREER_IDENTITY_MODE: 'agent' }, encoding: 'utf8', timeout: 70_000,
      });
      // Discard stderr and unstructured output; never capture the environment.
      let value = null;
      if (result.status === 0) { try { value = JSON.parse(result.stdout); } catch {} }
      const reply = `${dir}/${file.replace('.request.', '.reply.')}`;
      await writeFile(`${reply}.tmp`, JSON.stringify({ code: result.status, value }), { mode: 0o600 });
      await rename(`${reply}.tmp`, reply);
      await rename(`${dir}/${file}`, `${dir}/${file}.done`);
    }
    await new Promise(resolve => setTimeout(resolve, 100));
  }
} finally { await cleanup(); }
