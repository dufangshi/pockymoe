import { test, expect, type Page } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { waitForThread } from './helpers';

// Phone-screenshot-sized JPEGs, drawn in the browser so the test needs no image library.
async function screenshots(page: Page, count: number) {
  const urls = await page.evaluate((count) => Array.from({ length: count }, (_, index) => {
    const canvas = document.createElement('canvas');
    canvas.width = 1264;
    canvas.height = 2780;
    const context = canvas.getContext('2d')!;
    const gradient = context.createLinearGradient(0, 0, 1264, 2780);
    gradient.addColorStop(0, `hsl(${index * 90}, 70%, 60%)`);
    gradient.addColorStop(1, `hsl(${index * 90 + 45}, 70%, 30%)`);
    context.fillStyle = gradient;
    context.fillRect(0, 0, 1264, 2780);
    context.fillStyle = '#fff';
    context.font = '42px sans-serif';
    for (let line = 0; line < 60; line += 1) context.fillText(`Screenshot ${index} line ${line} ${'lorem ipsum '.repeat(3)}`, 40, 60 + line * 45);
    const noise = context.getImageData(0, 1400, 1264, 1380);
    for (let i = 0; i < noise.data.length; i += 4) noise.data[i] = (noise.data[i]! + ((i * 2654435761) >>> 27)) & 255;
    context.putImageData(noise, 0, 1400);
    return canvas.toDataURL('image/jpeg', 0.92);
  }), count);
  return urls.map((url) => Buffer.from(url.slice(url.indexOf(',') + 1), 'base64'));
}

interface ImageLoad { url: string; status: number; fromCache: boolean; bytes: number; finished: boolean }

async function watchImages(page: Page) {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Network.enable');
  const loads = new Map<string, ImageLoad>();
  cdp.on('Network.responseReceived', (event) => {
    if (!event.response.url.includes('/assets/image')) return;
    loads.set(event.requestId, {
      url: event.response.url,
      status: event.response.status,
      fromCache: Boolean(event.response.fromDiskCache || event.response.fromMemoryCache),
      bytes: 0,
      finished: false,
    });
  });
  const servedFromCache = new Set<string>();
  cdp.on('Network.requestServedFromCache', (event) => { servedFromCache.add(event.requestId); });
  cdp.on('Network.loadingFinished', (event) => {
    const load = loads.get(event.requestId);
    if (load) Object.assign(load, { bytes: event.encodedDataLength, finished: true, fromCache: load.fromCache || servedFromCache.has(event.requestId) });
  });
  return () => [...loads.values()];
}

test('chat photos load as thumbnails, the lightbox loads the original, and reloads reuse the cache', async ({ page, request }, testInfo) => {
  test.skip(testInfo.project.name !== 'desktop-chromium', 'Uses the Chromium DevTools protocol to count transferred bytes');
  const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Photo thumbnails' } })).json();
  const started = await (await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'Photo thumbnails', provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  const id = started.id ?? started.thread.id;

  await page.goto('about:blank');
  const photos = await screenshots(page, 3);
  const originalBytes = photos.reduce((sum, photo) => sum + photo.length, 0);
  const placeholders = photos.map((_, index) => `[PHOTO screen-${index}]`);
  const multipart: Record<string, string | { name: string; mimeType: string; buffer: Buffer }> = {
    prompt: `Look at these ${placeholders.join(' ')}`,
    attachmentManifest: JSON.stringify(photos.map((_, index) => ({ kind: 'photo', originalName: `screen-${index}.jpg`, placeholder: placeholders[index] }))),
  };
  photos.forEach((buffer, index) => { multipart[`file${index}`] = { name: `screen-${index}.jpg`, mimeType: 'image/jpeg', buffer }; });
  const prompt = await request.post(`${base}/api/threads/${id}/prompt`, { multipart });
  expect(prompt.ok(), await prompt.text()).toBeTruthy();
  await waitForThread(base, id);

  const images = await watchImages(page);
  await page.goto(`/threads/${id}`);
  const later = page.getByRole('button', { name: 'Later', exact: true });
  await later.waitFor({ timeout: 3_000 }).catch(() => {});
  if (await later.isVisible()) await later.click();
  const chips = page.locator('.thread-graph-zoomable-image-trigger img');
  await expect(chips).toHaveCount(3);
  await expect.poll(() => chips.evaluateAll((list) => list.filter((image) => (image as HTMLImageElement).naturalWidth > 0).length)).toBe(3);
  const sizes = await chips.evaluateAll((list) => list.map((image) => [(image as HTMLImageElement).naturalWidth, (image as HTMLImageElement).naturalHeight]));
  expect(sizes).toEqual([[218, 480], [218, 480], [218, 480]]);
  await expect.poll(() => images().filter((load) => load.finished).length).toBe(3);
  const first = images();
  expect(first.every((load) => load.url.includes('variant=thumbnail'))).toBe(true);
  const thumbnailBytes = first.reduce((sum, load) => sum + load.bytes, 0);
  expect(thumbnailBytes).toBeLessThan(originalBytes / 5);

  // The lightbox asks for the original, at full resolution.
  await page.getByRole('button', { name: 'Open image preview: screen-0', exact: false }).first().click();
  const full = page.locator('.thread-graph-image-lightbox img');
  await expect.poll(() => full.evaluate((image) => (image as HTMLImageElement).naturalWidth)).toBe(1264);
  expect(await full.getAttribute('src')).not.toContain('variant=');
  await expect.poll(() => images().filter((load) => !load.url.includes('variant=') && load.finished).length).toBe(1);
  const lightboxBytes = images().find((load) => !load.url.includes('variant='))!.bytes;
  expect(lightboxBytes).toBeGreaterThan(photos[0]!.length);
  await page.keyboard.press('Escape');

  // Photos are immutable: a reload takes them from the browser cache.
  const before = images().length;
  await page.reload();
  await expect.poll(() => chips.evaluateAll((list) => list.filter((image) => (image as HTMLImageElement).naturalWidth > 0).length)).toBe(3);
  await expect.poll(() => images().slice(before).every((load) => load.finished)).toBe(true);
  const reload = images().slice(before);
  expect(reload.every((load) => load.fromCache && load.status === 200), JSON.stringify(reload)).toBe(true);
  const reloadBytes = reload.reduce((sum, load) => sum + load.bytes, 0);
  expect(reloadBytes).toBe(0);

  const summary = { originalBytes, thumbnailBytes, lightboxBytes, reloadRequests: reload.length, reloadFromCache: reload.filter((load) => load.fromCache).length, reloadBytes };
  console.log(`photo-bytes ${JSON.stringify(summary)}`);
  await testInfo.attach('photo-bytes', { body: JSON.stringify(summary, null, 2), contentType: 'application/json' });
});
