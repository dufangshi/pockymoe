import { test, expect } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir, writeFile, readFile, copyFile } from 'node:fs/promises';
import path from 'node:path';

test.use({ actionTimeout: 15_000 });
const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
test('Markdown illustration back restores the document and reading position, with forward and file-list navigation', async ({ page, request, isMobile }, testInfo) => {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(path.join(absPath, 'docs/images'), { recursive: true });
  await writeFile(path.join(absPath, 'docs/index.md'), '# Screenshot guide\n\n' + Array.from({length:35}, (_,i) => `## Step ${i+1}\n\nRead this instruction and try the highlighted control.\n\n`).join('') + '[Open illustration](images/plot.png)\n\n[Read details](details.md)\n');
  await writeFile(path.join(absPath, 'docs/details.md'), '# Detailed instructions\n\nUse Back to continue reading the screenshot guide.');
  await writeFile(path.join(absPath, 'docs/images/plot.png'), Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aOCcAAAAASUVORK5CYII=', 'base64'));
  const workspace = await (await request.post(`${base}/api/workspaces`, {data:{absPath,label:'Document navigation'}})).json();
  const response = await request.post(`${base}/api/threads/start`, {data:{workspaceId:workspace.id,title:'Read tutorial screenshots',provider:'acp',agentId:'codex',model:'ios-e2e-stream',approvalMode:'yolo'}});
  expect(response.ok()).toBeTruthy();
  const value = await response.json();
  const id = value.id ?? value.thread.id;
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex-theme-mode','dark');
    localStorage.setItem('pockymoe.onboarding.v1:'+JSON.stringify([location.origin,'local:owner']), JSON.stringify({welcomeDismissed:true,completed:[],resume:{}}));
  });
  await page.goto(`/threads/${id}`);
  await page.locator(isMobile ? '.matter-topbar' : '.matter-rail').getByRole('button',{name:'Toggle Explorer',exact:true}).click();
  const files = page.getByTestId('workspace-panel');
  const folder = files.getByRole('treeitem',{name:'docs',exact:true}).getByRole('button',{name:'docs',exact:true});
  if (isMobile) await folder.tap(); else await folder.dblclick();
  await files.getByRole('treeitem',{name:'index.md',exact:true}).getByRole('button',{name:'index.md',exact:true}).click();
  const markdown = files.locator('.thread-graph-markdown-preview');
  await expect(markdown.getByRole('heading',{name:'Screenshot guide',exact:true})).toBeVisible();
  await expect(page.locator('.workbench-tool-drawer > header')).toHaveCount(0);
  await expect(files.locator('.thread-graph-editor-breadcrumbs')).toHaveCount(0);
  await expect(page.locator('.thread-graph-right-tabs')).toHaveCount(0);
  await expect(files.getByRole('button',{name:'Check disk',exact:true})).toHaveCount(0);
  const toolbar = files.locator('.workspace-file-toolbar');
  expect((await toolbar.boundingBox())!.height).toBeLessThanOrEqual(48);
  expect((await markdown.boundingBox())!.height).toBeGreaterThan(page.viewportSize()!.height * .6);
  await files.getByRole('button',{name:'File actions',exact:true}).click();
  const actions = files.getByRole('toolbar',{name:'File actions',exact:true});
  await actions.getByRole('button',{name:'Reload from disk',exact:true}).click();
  await expect(actions.locator('.workspace-file-action-feedback[role="status"]')).toHaveText('Already up to date');
  await page.keyboard.press('Escape');
  await expect(actions).toHaveCount(0);
  await expect(markdown).toBeVisible();
  await markdown.getByRole('link',{name:'Open illustration',exact:true}).scrollIntoViewIfNeeded();
  const before = await markdown.evaluate(el => el.scrollTop);
  expect(before).toBeGreaterThan(100);
  await markdown.getByRole('link',{name:'Open illustration',exact:true}).click();
  await expect(files.locator('.thread-graph-viewer img')).toBeVisible();
  await files.getByRole('button',{name:'Back to index.md',exact:true}).click();
  await expect(markdown).toBeVisible();
  await expect.poll(async () => Math.abs(await markdown.evaluate(el => el.scrollTop) - before)).toBeLessThan(3);
  await expect(markdown.getByRole('link',{name:'Open illustration',exact:true})).toBeInViewport();
  await page.screenshot({path:testInfo.outputPath('markdown-position-restored.png'),scale:'css'});
  await files.getByRole('button',{name:'File actions',exact:true}).click();
  await files.getByRole('button',{name:'Forward',exact:true}).click();
  await expect(files.locator('.thread-graph-viewer img')).toBeVisible();
  await files.getByRole('button',{name:'Back to index.md',exact:true}).click();
  await markdown.getByRole('link',{name:'Read details',exact:true}).click();
  await expect(markdown.getByRole('heading',{name:'Detailed instructions',exact:true})).toBeVisible();
  await expect(files.getByRole('button',{name:'Forward',exact:true})).toHaveCount(0);
  await files.getByRole('button',{name:'Back to index.md',exact:true}).click();
  await expect(markdown.getByRole('link',{name:'Open illustration',exact:true})).toBeInViewport();
  if (isMobile) await files.getByRole('button',{name:'Back to files',exact:true}).click();
  await expect(files.getByRole('tree',{name:'Workspace files'})).toBeVisible();
  await expect(files.getByRole('treeitem',{name:'docs',exact:true})).toHaveAttribute('aria-expanded','true');
  if (isMobile) {
    await files.getByRole('treeitem',{name:'index.md',exact:true}).getByRole('button',{name:'index.md',exact:true}).click();
    await markdown.getByRole('link',{name:'Open illustration',exact:true}).click();
    await expect(files.locator('.thread-graph-viewer img')).toBeVisible();
    await files.getByRole('button',{name:'Back to index.md',exact:true}).click();
    await expect(markdown.getByRole('link',{name:'Open illustration',exact:true})).toBeInViewport();
  }
});

test('file previews retain multiple tabs and both workspace and chat images accept pinch gestures', async ({ page, request, context, isMobile }, testInfo) => {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(absPath, { recursive: true });
  const picture = '<svg xmlns="http://www.w3.org/2000/svg" width="600" height="400"><rect width="600" height="400" fill="#16372c"/><circle cx="300" cy="200" r="110" fill="#13bc85"/><text x="300" y="210" text-anchor="middle" font-size="32" fill="white">Pinch preview</text></svg>';
  await writeFile(path.join(absPath, 'first.svg'), picture);
  await writeFile(path.join(absPath, 'second.svg'), picture.replace('Pinch preview', 'Second image'));
  await writeFile(path.join(absPath, 'notes.md'), '# Kept open\n\nTabs survive returning to the file tree.');
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'Image tabs' } })).json();
  const started = await (await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'Image tabs', provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  const id = started.id ?? started.thread.id;
  const photoPath = `./.temp/threads/${id}/photo.png`;
  await mkdir(path.join(absPath, '.temp/threads', id), { recursive: true });
  await copyFile('apps/supervisor-web/public/icon-192.png', path.join(absPath, photoPath));
  const detail = await (await request.get(`${base}/api/threads/${id}`)).json();
  const now = new Date().toISOString();
  await page.route(`**/api/threads/${id}?**`, route => route.fulfill({ json: { ...detail, totalTurnCount: 1, turns: [{ id: 'photo-turn', status: 'completed', startedAt: now, completedAt: now, items: [{ id: 'photo', kind: 'userMessage', text: `[PHOTO ${photoPath}]` }, { id: 'reply', kind: 'agentMessage', text: 'Preview this image.' }] }] } }));
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex-theme-mode', 'dark');
    localStorage.setItem('pockymoe.onboarding.v1:' + JSON.stringify([location.origin, 'local:owner']), JSON.stringify({ welcomeDismissed: true, completed: [] }));
  });
  await page.goto(`/threads/${id}`);
  const toolbar = page.locator(isMobile ? '.matter-topbar' : '.matter-rail');
  await toolbar.getByRole('button', { name: 'Toggle Explorer', exact: true }).click();
  const files = page.getByTestId('workspace-panel');
  const open = async (name: string) => {
    await files.getByRole('treeitem', { name, exact: true }).getByRole('button', { name, exact: true }).click();
    await expect(files.getByRole('tab', { name, exact: true })).toHaveAttribute('aria-selected', 'true');
  };
  await open('first.svg');
  await expect(files.locator('.workspace-image-viewport img')).toBeVisible();
  if (isMobile) await files.getByRole('button', { name: 'Back to files', exact: true }).click();
  else await files.getByRole('button', { name: 'Back to files', exact: true }).click();
  await expect(files.getByRole('tree')).toBeVisible();
  await open('second.svg');
  await expect(files.getByRole('tab')).toHaveCount(2);
  // Returning to the tree retains the file tab and drawer.
  await files.getByRole('button', { name: 'Back to files', exact: true }).click();
  await expect(files.getByRole('tree')).toBeVisible();
  await open('notes.md');
  await expect(files.getByRole('tab')).toHaveCount(3);
  await files.getByRole('tab', { name: 'first.svg', exact: true }).click();
  await expect(files.locator('.workspace-image-viewport img')).toBeVisible();
  const stage = files.locator('.workspace-image-viewport');
  const percent = files.locator('.workspace-image-controls').getByRole('button', { name: 'Reset zoom', exact: true });
  if (isMobile) {
    const cdp = await context.newCDPSession(page);
    async function pinch(target: import('@playwright/test').Locator) {
      const box = (await target.boundingBox())!;
      const x = box.x + box.width / 2, y = box.y + box.height / 2;
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ id: 1, x: x - 30, y }, { id: 2, x: x + 30, y }] });
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [{ id: 1, x: x - 70, y }, { id: 2, x: x + 70, y }] });
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
    }
    await pinch(stage);
    await expect(percent).not.toHaveText('100%');
    await expect.poll(async () => parseInt((await percent.textContent())!)).toBeGreaterThan(150);
    expect(await page.evaluate(() => visualViewport!.scale)).toBe(1);
    await page.screenshot({ path: testInfo.outputPath('multi-tab-pinched-file.png'), scale: 'css' });
    await files.getByRole('button', { name: 'Open image preview', exact: true }).click();
    const lightbox = page.locator('.thread-graph-image-lightbox');
    await pinch(lightbox.locator('.thread-graph-image-lightbox-viewport'));
    await expect(lightbox.locator('.thread-graph-image-lightbox-scale')).not.toContainText('100%');
    await lightbox.getByRole('button', { name: 'Close image preview', exact: true }).click();
    await percent.click();
    await expect(percent).toHaveText('100%');
    await files.getByRole('button', { name: 'Back to files', exact: true }).click();
    await files.getByTestId('workbench-close-files').click();
    const photo = page.getByRole('button', { name: 'Open image preview: photo.png', exact: true });
    await expect.poll(() => photo.locator('img').evaluate((img: HTMLImageElement) => img.complete && img.naturalWidth > 0)).toBe(true);
    await photo.click();
    await expect(lightbox).toBeVisible();
    await expect.poll(() => lightbox.locator('img').evaluate((img: HTMLImageElement) => img.complete && img.naturalWidth > 0)).toBe(true);
    await pinch(lightbox.locator('.thread-graph-image-lightbox-viewport'));
    await expect(lightbox.locator('.thread-graph-image-lightbox-scale')).not.toContainText('100%');
    await page.screenshot({ path: testInfo.outputPath('pinched-chat-photo.png'), scale: 'css' });
    await lightbox.getByRole('button', { name: 'Close image preview', exact: true }).click();
    await toolbar.getByRole('button', { name: 'Toggle Explorer', exact: true }).click();
    await files.getByTestId('expand-viewer').click();
  } else {
    await files.getByRole('button', { name: 'Zoom in', exact: true }).click();
    await expect(percent).toHaveText('125%');
    await stage.hover(); await page.mouse.wheel(0, -100);
    await expect(percent).toHaveText('150%');
    await page.screenshot({ path: testInfo.outputPath('multi-tab-desktop-file.png'), scale: 'css' });
  }
  await expect(files.getByRole('tab')).toHaveCount(3);
  await files.getByRole('button', { name: 'Close second.svg', exact: true }).click();
  await expect(files.getByRole('tab')).toHaveCount(2);
  await files.getByRole('tab', { name: 'notes.md', exact: true }).click();
  await expect(files.getByRole('heading', { name: 'Kept open', exact: true })).toBeVisible();
  await files.getByRole('tab', { name: 'first.svg', exact: true }).click();
  await expect(files.locator('.workspace-image-viewport img')).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(page.viewportSize()!.width);
});


test('GitHub README HTML renders centered badges and workspace illustrations', async ({ page, request, isMobile }, testInfo) => {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
  await mkdir(path.join(absPath, 'docs/assets/readme'), { recursive: true });
  await mkdir(path.join(absPath, 'apps/supervisor-web/public'), { recursive: true });
  const content = (await readFile('README.md', 'utf8')).split('<p align="center">\n  <a href="#quick-start">')[0]!
    + '\n<details><summary>More information</summary>\n\n**Keep reading** in the workspace.\n\n</details>';
  await writeFile(path.join(absPath, 'README.md'), content);
  await writeFile(path.join(absPath, 'README.zh-CN.md'), '# 中文说明');
  await copyFile('docs/assets/readme/hero.jpg', path.join(absPath, 'docs/assets/readme/hero.jpg'));
  await copyFile('apps/supervisor-web/public/icon-192.png', path.join(absPath, 'apps/supervisor-web/public/icon-192.png'));
  // The actual README fixture uses remote badges; keep the test offline and deterministic.
  await page.route('https://img.shields.io/**', route => route.fulfill({ contentType: 'image/svg+xml', body: '<svg xmlns="http://www.w3.org/2000/svg" width="94" height="24"><rect width="94" height="24" rx="3" fill="#73508a"/><text x="47" y="16" text-anchor="middle" fill="white" font-size="12">README</text></svg>' }));
  const workspace = await (await request.post(`${base}/api/workspaces`, { data: { absPath, label: 'README HTML' } })).json();
  const started = await (await request.post(`${base}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'README preview', provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } })).json();
  await page.addInitScript(() => {
    localStorage.setItem('remote-codex-theme-mode', 'dark');
    localStorage.setItem('pockymoe.onboarding.v1:' + JSON.stringify([location.origin, 'local:owner']), JSON.stringify({ welcomeDismissed: true, completed: [] }));
  });
  await page.goto(`/threads/${started.id ?? started.thread.id}`);
  await page.locator(isMobile ? '.matter-topbar' : '.matter-rail').getByRole('button', { name: 'Toggle Explorer', exact: true }).click();
  const files = page.getByTestId('workspace-panel');
  await files.getByRole('treeitem', { name: 'README.md', exact: true }).getByRole('button', { name: 'README.md', exact: true }).click();
  const markdown = files.locator('.thread-graph-markdown-preview');
  await expect(markdown.locator('h1')).toHaveText('Pockymoe');
  await expect(markdown.locator('p[align="center"]').first()).toHaveCSS('text-align', 'center');
  await expect.poll(() => markdown.locator('img[alt^="Pockymoe:"]').evaluate((img: HTMLImageElement) => img.complete && img.naturalWidth > 0)).toBe(true);
  await expect(markdown.locator('h1 img')).toHaveCSS('width', '44px');
  await expect(markdown.locator('a button')).toHaveCount(0);
  expect(await markdown.textContent()).not.toContain('<p align=');
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(page.viewportSize()!.width);
  await page.screenshot({ path: testInfo.outputPath(`readme-html-${isMobile ? 'mobile' : 'desktop'}.png`), scale: 'css' });
  await markdown.getByRole('button', { name: /^Open image preview: Pockymoe:/ }).click();
  const lightbox = page.locator('.thread-graph-image-lightbox');
  await expect(lightbox).toBeVisible();
  await lightbox.getByRole('button', { name: 'Zoom in', exact: true }).click();
  await expect(lightbox.locator('.thread-graph-image-lightbox-scale')).toContainText('125%');
  await lightbox.evaluate(el => el.setAttribute('data-kept-preview', 'readme'));
  // Each thread refresh re-renders the parent page. The same modal and zoom
  // must survive consecutive refreshes, rather than disappearing a few seconds later.
  for (let i = 0; i < 2; i++) {
    const refreshed = page.waitForResponse(response => /\/api\/threads\/[^/?]+\?view=summary/.test(response.url()) && response.ok());
    await page.evaluate(() => window.dispatchEvent(new Event('model-pricing-updated')));
    await refreshed;
    await expect(lightbox).toHaveAttribute('data-kept-preview', 'readme');
    await expect(lightbox.locator('.thread-graph-image-lightbox-scale')).toContainText('125%');
  }
  await page.screenshot({ path: testInfo.outputPath('readme-image-stays-open.png'), scale: 'css' });
  await lightbox.getByRole('button', { name: 'Close image preview', exact: true }).click();
  await expect(lightbox).toHaveCount(0);
  await markdown.getByText('More information', { exact: true }).click();
  await expect(markdown.getByText('Keep reading', { exact: true })).toBeVisible();
  await markdown.getByRole('link', { name: '简体中文', exact: true }).click();
  await expect(files.getByRole('heading', { name: '中文说明', exact: true })).toBeVisible();
  await expect(files.getByRole('tab')).toHaveCount(2);
  await files.getByRole('button', { name: 'Back to README.md', exact: true }).click();
  await expect(markdown.locator('h1')).toHaveText('Pockymoe');
});
