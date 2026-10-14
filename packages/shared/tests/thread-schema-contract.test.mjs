import { spawnSync } from 'node:child_process';
import assert from 'node:assert/strict';
import { test } from 'node:test';

// Reuse the Rust contract check: no generated fixture or second inventory to drift.
test('shared thread discriminants match the Rust protocol', () => {
  const result = spawnSync('cargo', [
    'test', '-p', 'pockymoe-protocol', '--locked', '--test', 'thread_schema_contract',
  ], {
    cwd: new URL('../../../', import.meta.url),
    stdio: 'inherit',
  });
  assert.ifError(result.error);
  assert.equal(result.status, 0, 'Rust/TS thread schema contract check failed');
});
