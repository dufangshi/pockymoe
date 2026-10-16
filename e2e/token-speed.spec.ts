import { test, expect } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';

test('real GPT-6.1 Sol shows live throughput, excludes tool wait, and retains speed and billing after reload', async ({ page, request }) => {
  test.skip(process.env.E2E_REAL_CODEX !== '1', 'Explicit real-token acceptance only');
  test.setTimeout(300_000);
  const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, `speed-${randomUUID()}`);
  await mkdir(absPath, { recursive: true });
  const wsResponse = await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Real token speed acceptance' } });
  expect(wsResponse.ok()).toBeTruthy();
  const workspace = await wsResponse.json();
  const created = await request.post(`${base}/api/threads/start`, { data: {workspaceId:workspace.id,title:'GPT-6.1 Sol token speed acceptance',provider:'codex',model:'gpt-6.1-sol',reasoningEffort:'low',approvalMode:'yolo'} });
  expect(created.ok()).toBeTruthy();
  const result = await created.json();
  const id = result.id ?? result.thread.id;
  const detail = async () => (await request.get(`${base}/api/threads/${id}`)).json();
  try {
    await page.goto(`/threads/${id}`);
    await page.getByRole('textbox', { name: 'Prompt' }).fill('This is a token-speed acceptance test. Do not modify files, create agents, or use the pockymoe CLI. First write 200 English words explaining output token throughput. Then use exec_command to run exactly sleep 12 with yield_time_ms=10000, and wait until it has finished (poll with yield_time_ms=10000 if the tool returns a session ID). Then perform a SECOND separate exec_command running sleep 12, again yield_time_ms=10000 and wait for completion before continuing. Do not combine these calls. Finally write another 200 English words explaining why tool time must be excluded. End with TOKEN_SPEED_ACCEPTANCE_DONE. These steps, in order, are required.');
    await page.getByRole('button', { name: 'Send Prompt', exact: true }).click();
    const liveSpeed = page.locator('.thread-graph-turn-footer [data-testid="turn-token-speed"]');
    await expect(liveSpeed).toBeVisible();
    await expect(liveSpeed).toHaveAttribute('title', /Output tokens per second/);
    await expect.poll(async () => (await detail()).turns.at(-1)?.tokenUsage?.generationSpeed?.state, { timeout: 150_000 }).toBe('tool');
    const samples: { llmTimeMs:number; outputTokens:number }[] = [];
    await expect.poll(async () => {
      const speed = (await detail()).turns.at(-1)?.tokenUsage?.generationSpeed;
      if (speed?.state === 'tool' && speed.outputTokens > 0) {
        // Codex reports request usage after tool output. Observe a subsequent
        // full tool phase once real counters exist; do not fake first-call tokens.
        if (samples.length && samples[0].llmTimeMs !== speed.llmTimeMs) samples.length = 0;
        samples.push(speed);
      }
      return samples.length;
    }, { intervals:[500,500,500], timeout:90_000 }).toBeGreaterThanOrEqual(4);
    expect(Math.max(...samples.map(s => s.llmTimeMs)) - Math.min(...samples.map(s => s.llmTimeMs))).toBeLessThan(100);
    await expect(liveSpeed).toHaveText(/\d[\d,.]*/);
    await expect.poll(async () => (await detail()).turns.at(-1)?.status, {timeout:150_000}).toBe('completed');
    const completed = (await detail()).turns.at(-1);
    const speed = completed.tokenUsage.generationSpeed;
    expect(completed.model).toBe('gpt-6.1-sol');
    expect(completed.tokenUsage.total.outputTokens).toBeGreaterThan(0);
    expect(speed.outputTokens).toBe(completed.tokenUsage.total.outputTokens);
    expect(speed.active).toBe(false);
    expect(speed.averageTokensPerSecond).toBeCloseTo(speed.outputTokens / (speed.llmTimeMs / 1000), 5);
    const wallMs = Date.parse(completed.completedAt) - Date.parse(completed.startedAt);
    expect(wallMs - speed.llmTimeMs).toBeGreaterThan(8_000);
    expect(completed.priceEstimate.pricingModelKey).toBe('gpt-6.1-sol');
    expect(completed.priceEstimate.totalUsd).toBeGreaterThan(0);
    const tokens = completed.tokenUsage.total;
    const expectedUsd = ((tokens.inputTokens - tokens.cachedInputTokens - (tokens.cacheWriteInputTokens ?? 0)) * 2 + tokens.cachedInputTokens * .1 + (tokens.cacheWriteInputTokens ?? 0) * 2.5 + tokens.outputTokens * 10) / 1e6;
    expect(completed.priceEstimate.totalUsd).toBeCloseTo(expectedUsd, 8);
    await page.reload();
    const summary = page.locator('.thread-turn-receipt');
    await expect(summary.getByTestId('turn-token-speed')).toHaveText(/\d[\d,.]*/);
    await expect(summary.locator('.thread-turn-usage-price')).toBeVisible();
    await page.setViewportSize({width:390,height:844});
    await expect(summary.getByTestId('turn-token-speed')).toBeVisible();
    const box = await summary.getByTestId('turn-token-speed').boundingBox();
    expect(box).not.toBeNull();
    expect(box!.x + box!.width).toBeLessThanOrEqual(390);
    await expect(page.getByText(/TOKEN_SPEED_ACCEPTANCE_DONE/).last()).toBeVisible();
    expect((await detail()).turns.at(-1).tokenUsage.generationSpeed).toEqual(speed);
    console.log(JSON.stringify({model:completed.model,outputTokens:speed.outputTokens,llmSeconds:speed.llmTimeMs/1000,wallSeconds:wallMs/1000,averageTokensPerSecond:speed.averageTokensPerSecond,usd:completed.priceEstimate.totalUsd}));
  } finally {
    await request.post(`${base}/api/threads/${id}/interrupt`, {data:{}});
  }
});
