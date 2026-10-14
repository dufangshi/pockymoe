import { test, expect } from '@playwright/test';
import { mkdir, writeFile, readFile } from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
import { deflateRawSync } from 'node:zlib';
import path from 'node:path';
import { drawioPreviewDocument } from '../pockymoe-thread-ui/packages/thread-ui/src/components/graph-workspace/GraphDrawioPreview';

const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
test.use({ actionTimeout: 15_000 });
const model = (label: string) => `<mxGraphModel><root><mxCell id="0"/><mxCell id="1" parent="0"/><mxCell id="2" value="${label}" style="rounded=1;whiteSpace=wrap;html=1;" vertex="1" parent="1"><mxGeometry x="40" y="40" width="180" height="80" as="geometry"/></mxCell></root></mxGraphModel>`;

test('draw.io renders under the public relay CSP without inline scripts or eval', async ({ page }) => {
  // srcdoc inherits the public site's HTTP CSP. A meta policy cannot relax it.
  // Serve a minimal parent instead of Vite's development-only inline preamble.
  const csp = "default-src 'self'; script-src 'self' 'sha256-J/u35pSjfPx+3XnEQrievRBlloQ59CziqpHF08KUmhU='; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; worker-src 'self'; frame-src 'self' blob:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'";
  // Use a public HTTPS origin: fulfilling a localhost navigation gives Chromium
  // a synthetic public address space and incorrectly triggers loopback access checks.
  await page.route('https://drawio-csp.example/**', async (route) => {
    const pathname = new URL(route.request().url()).pathname;
    if (pathname === '/') return route.fulfill({
      contentType: 'text/html', headers: { 'content-security-policy': csp }, body: '<!doctype html><html><body></body></html>',
    });
    if (!['/vendor/drawio/bootstrap.v1.js', '/vendor/drawio/viewer-static.v32.3.0.min.js'].includes(pathname)) return route.abort();
    await route.fulfill({ contentType: 'text/javascript', body: await readFile(path.resolve('apps/supervisor-web/public' + pathname)) });
  });
  await page.goto('https://drawio-csp.example/');
  const xml = `<mxfile><diagram name="Architecture">${model('数据库 / Architecture')}</diagram></mxfile>`;
  const srcdoc = drawioPreviewDocument(xml, new URL(page.url()).origin);
  await page.evaluate((srcdoc) => {
    const frame = document.createElement('iframe');
    frame.title = 'CSP diagram';
    frame.setAttribute('sandbox', 'allow-scripts');
    frame.style.cssText = 'width:90vw;height:80vh';
    frame.srcdoc = srcdoc;
    document.body.append(frame);
  }, srcdoc);
  const frame = page.frameLocator('iframe');
  await expect(frame.locator('svg')).toBeVisible();
  await expect(frame.getByText('数据库 / Architecture', { exact: true })).toBeVisible();
});

test('Explorer renders complete draw.io files, compressed pages and source on desktop and mobile', async ({ page, request }, testInfo) => {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, `drawio-${randomUUID()}`);
  await mkdir(absPath, { recursive: true });
  const compressed = deflateRawSync(Buffer.from(encodeURIComponent(model('Compressed second page')))).toString('base64');
  // The second page is beyond the old text-preview endpoint's 64 KiB limit.
  const xml = `<mxfile><diagram id="first" name="First">${model('Architecture root')}</diagram><!--${'padding'.repeat(11000)}--><diagram id="second" name="Second">${compressed}</diagram></mxfile>`;
  await writeFile(path.join(absPath, 'architecture.drawio'), xml);
  await writeFile(path.join(absPath, 'invalid.dio'), '<mxfile><broken>');
  const ws = await (await request.post(base + '/api/workspaces', { data: { absPath, label: 'Draw.io preview' } })).json();
  const t = await (await request.post(base + '/api/threads/start', { data: { workspaceId: ws.id, title: 'Draw.io preview', provider: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  const externalRequests: string[] = [];
  page.on('request', (request) => {
    if (/diagrams\.net|draw\.io/.test(new URL(request.url()).hostname)) externalRequests.push(request.url());
  });
  await page.goto(`/threads/${t.id ?? t.thread.id}`);
  if (testInfo.project.name === 'mobile-chromium') {
    await page.getByRole('button', { name: 'Toggle shortcuts sidebar', exact: true }).click();
    await page.getByRole('button', { name: 'Open Explorer', exact: true }).click();
  } else {
    await page.getByRole('navigation', { name: 'Workspace tools' }).getByRole('button', { name: 'Toggle Explorer' }).click();
  }
  // Use the tree's keyboard activation: hover actions can cover the label in a
  // narrow desktop Explorer, independently of the diagram renderer.
  await page.getByRole('treeitem', { name: 'architecture.drawio', exact: true }).press('Enter');
  const frame = page.frameLocator('iframe[title="Draw.io preview: architecture.drawio"]');
  await expect(frame.locator('svg')).toBeVisible();
  await expect(frame.getByText('Architecture root', { exact: true })).toBeVisible();
  await frame.getByTitle('Next Page', { exact: true }).click();
  await expect(frame.getByText('Compressed second page', { exact: true })).toBeVisible();
  const shape = frame.locator('svg rect').first();
  const widthBeforeZoom = (await shape.boundingBox())!.width;
  await frame.getByTitle('Zoom In', { exact: true }).click();
  await expect.poll(async () => (await shape.boundingBox())!.width).toBeGreaterThan(widthBeforeZoom);
  await page.getByRole('button', { name: 'Diagram source', exact: true }).click();
  await expect(page.locator('iframe[title="Draw.io preview: architecture.drawio"]')).toHaveCount(0);
  await expect(page.locator('.monaco-editor, [aria-label="Source code"]')).toBeVisible();
  await expect(page.locator('.monaco-editor, [aria-label="Source code"]')).toContainText('mxfile');
  await page.getByRole('button', { name: 'Diagram preview', exact: true }).click();
  await expect(frame.getByText('Architecture root', { exact: true })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('drawio-preview.png') });
  if (testInfo.project.name === 'mobile-chromium') {
    await page.getByRole('button', { name: 'Show Explorer', exact: true }).click();
  }
  await page.getByRole('treeitem', { name: 'invalid.dio', exact: true }).press('Enter');
  await expect(page.frameLocator('iframe[title="Draw.io preview: invalid.dio"]').getByRole('alert')).toContainText('Invalid draw.io XML');
  expect(externalRequests).toEqual([]);
});
