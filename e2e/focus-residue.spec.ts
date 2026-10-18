import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { expect, test, type Locator } from '@playwright/test';

const api = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
const appearance = (locator: Locator) => locator.evaluate(element => {
  const style = getComputedStyle(element);
  return [style.color, style.backgroundColor, style.borderColor, style.boxShadow, style.transform];
});

for (const preset of ['classic', 'plum-pocket', 'terminal']) {
  for (const mode of ['light', 'dark']) {
    test(`${preset} ${mode}: mouse feedback clears and keyboard/touch actions remain reachable`, async ({ page, request }, info) => {
      await page.addInitScript(({ preset, mode }) => {
        localStorage.setItem('pockymoe-theme-preset', preset);
        localStorage.setItem('remote-codex-theme-mode', mode);
        localStorage.setItem('remote-codex.locale', 'en');
        localStorage.setItem('pockymoe.phoneDock', 'off');
        localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
        // Keep clipboard permission/platform differences out of this focus test.
        Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async () => {} } });
      }, { preset, mode });
      const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID());
      await mkdir(absPath, { recursive: true });
      const workspaceResponse = await request.post(`${api}/api/workspaces`, { data: { absPath, label: 'Focus regression' } });
      expect(workspaceResponse.ok()).toBeTruthy();
      const workspace = await workspaceResponse.json();
      const started = await request.post(`${api}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'Focus regression', provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } });
      expect(started.ok()).toBeTruthy();
      const thread = await started.json();
      const id = thread.id ?? thread.thread.id;
      const detail = await (await request.get(`${api}/api/threads/${id}`)).json();
      await page.routeWebSocket(/\/ws(?:\?.*)?$/, socket => {
        socket.send(JSON.stringify({ type: 'supervisor.connected' }));
        socket.onMessage(() => socket.send(JSON.stringify({ type: 'supervisor.pong' })));
      });
      await page.route(`**/api/threads/${id}?**`, route => route.fulfill({ json: {
        ...detail, totalTurnCount: 1, activeSubagents: [],
        thread: { ...detail.thread, status: 'idle', activeTurnId: null },
        turns: [{ id: 'focus-turn', status: 'completed', startedAt: new Date().toISOString(), completedAt: new Date().toISOString(), items: [
          { id: 'prompt', kind: 'userMessage', text: 'Check focus feedback' },
          { id: 'reply', kind: 'agentMessage', text: 'Focus regression answer.\n\n```js\nconst value = 1;\n```' },
        ] }],
      } }));

      for (const width of [1280, 390]) {
        await page.setViewportSize({ width, height: 900 });
        await page.goto(`/threads/${id}`);
        await expect(page.locator('html')).toHaveAttribute('data-theme-preset', preset);
        await expect(page.locator('html')).toHaveAttribute('data-theme-effective', mode);
        const bubble = page.locator('.thread-graph-message-bubble.is-assistant').last();
        const actions = bubble.locator('.thread-graph-message-copy-desktop');
        const copy = actions.getByRole('button', { name: 'Copy agent reply', exact: true });
        await expect(bubble).toContainText('Focus regression answer.');
        const away = () => page.mouse.move(width - 2, 2);
        await away();
        await expect(actions).toHaveCSS('opacity', '0');
        const resting = await appearance(copy);
        await bubble.hover();
        await expect(actions).toHaveCSS('opacity', '1');
        await copy.click();
        await expect(copy).toHaveAttribute('data-state', 'copied');
        await expect(copy).toBeFocused();
        await away();
        // Focus stays on the clicked control: hiding must not depend on blur.
        await expect(copy).toBeFocused();
        expect(await copy.evaluate(element => element.matches(':focus-visible'))).toBe(false);
        await expect(actions).toHaveCSS('opacity', '0');
        await expect.poll(() => appearance(copy)).toEqual(resting);

        await bubble.locator('.thread-graph-message-content p').first().click();
        await away();
        await expect(bubble).toHaveAttribute('data-touch-actions', 'false');
        await expect(actions).toHaveCSS('opacity', '0');

        const code = bubble.locator('.thread-graph-code-block');
        const codeCopy = code.getByRole('button', { name: 'Copy code', exact: true });
        await code.hover();
        await codeCopy.click();
        await away();
        await expect(codeCopy).toBeFocused();
        await expect(codeCopy).toHaveCSS('opacity', '0');

        // Tab into an otherwise hidden affordance, without moving the pointer.
        await page.keyboard.press('Shift+Tab');
        await page.keyboard.press('Tab');
        await expect(codeCopy).toBeFocused();
        expect(await codeCopy.evaluate(element => element.matches(':focus-visible'))).toBe(true);
        await expect(codeCopy).toHaveCSS('opacity', '0.6');
        await expect(actions).toHaveCSS('opacity', '1');
        // Shiki's scrollable <pre> can be the next tab stop inside this block.
        // Move keyboard focus to the message action before checking it hides.
        await copy.focus();
        await expect(copy).toBeFocused();
        expect(await copy.evaluate(element => element.matches(':focus-visible'))).toBe(true);
        await expect(codeCopy).toHaveCSS('opacity', '0');

        const composer = page.getByTestId('chat-composer');
        // The folded composer shows Send once there is a draft. Pressing it and
        // releasing elsewhere sends nothing (no click), but leaves it focused.
        const prompt = composer.getByRole('textbox', { name: 'Prompt' });
        await prompt.fill('press check');
        await prompt.evaluate(element => (element as HTMLElement).blur());
        const button = composer.getByRole('button', { name: 'Send Prompt' });
        await expect(button).toBeEnabled();
        const shell = composer.locator('.thread-graph-composer-shell');
        // Read the resting look once blur transitions have finished.
        const settled = async (target: typeof shell) => {
          let previous = JSON.stringify(await appearance(target));
          for (;;) {
            await page.waitForTimeout(150);
            const next = JSON.stringify(await appearance(target));
            if (next === previous) return JSON.parse(next);
            previous = next;
          }
        };
        const shellResting = await settled(shell);
        const buttonResting = await settled(button);
        await button.hover();
        await page.mouse.down();
        expect(await button.evaluate(element => element.matches(':active'))).toBe(true);
        await away();
        await page.mouse.up();
        await expect(button).toBeFocused();
        await expect.poll(() => appearance(button)).toEqual(buttonResting);
        await expect.poll(() => appearance(shell)).toEqual(shellResting);
        await prompt.fill('');

        // A touch tap reveals and a second tap dismisses. Dispatch only the
        // pointer event here; desktop-chromium remains the one browser project.
        await bubble.dispatchEvent('pointerup', { pointerType: 'touch' });
        await expect(actions).toHaveCSS('opacity', '1');
        await bubble.dispatchEvent('pointerup', { pointerType: 'touch' });
        await expect(actions).toHaveCSS('opacity', '0');
        await bubble.hover();
        await expect(actions).toHaveCSS('opacity', '1');
        await page.screenshot({ path: info.outputPath(`${width}.png`), scale: 'css' });
      }
    });
  }
}
