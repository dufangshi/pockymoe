// This process MUST be started by Treer as a managed Agent, never by docker exec.
import { spawnSync } from 'node:child_process';
import { readdir, readFile, rename, writeFile } from 'node:fs/promises';
const dir = process.env.TREER_LAB_JOB_DIR ?? '/workspace/lab-jobs';
if (!process.env.TREER_AGENT_ID || !process.env.TREER_WORKLOAD_CREDENTIAL) process.exit(2);
await writeFile(`${dir}/ready.json`, JSON.stringify({ agentId: process.env.TREER_AGENT_ID }), { mode: 0o600 });
console.log('LAB_MANAGED_READY');
for (;;) {
  for (const file of await readdir(dir)) {
    if (!/^[a-f0-9-]+\.request\.json$/.test(file)) continue;
    const args = JSON.parse(await readFile(`${dir}/${file}`, 'utf8'));
    const result = spawnSync(process.env.TREER_LAB_CLI ?? '/usr/local/bin/treer', args, {
      encoding: 'utf8', env: { ...process.env, TREER_IDENTITY_MODE: 'agent' }, timeout: 60_000,
    });
    // Only successful structured CLI responses are returned; stderr is discarded.
    let value;
    try { value = result.status === 0 ? JSON.parse(result.stdout) : null; } catch { value = null; }
    await writeFile(`${dir}/${file.replace('.request.', '.reply.')}`, JSON.stringify({ code: result.status, value }), { mode: 0o600 });
    await rename(`${dir}/${file}`, `${dir}/${file}.done`);
  }
  await new Promise(resolve => setTimeout(resolve, 100));
}
