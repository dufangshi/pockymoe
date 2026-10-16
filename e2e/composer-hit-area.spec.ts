import { readFileSync } from 'node:fs';
import { expect, test } from '@playwright/test';

const workbenchCss = readFileSync('pockymoe-thread-ui/packages/thread-ui/src/styles/matter-workbench.css', 'utf8');

const composerCss = readFileSync('pockymoe-thread-ui/packages/thread-ui/src/styles/composer-plan.css', 'utf8');

// An isolated transcript/composer overlap: exercise browser hit testing without a harness.
test('floating timeline navigation lets transcript clicks through its empty strip', async ({ page }) => {
  for (const width of [1280, 390]) {
    await page.setViewportSize({ width, height: 800 });
    await page.setContent(`<style>
      ${workbenchCss}
      ${composerCss}
      .pointer-events-none { pointer-events: none; }
      .pointer-events-auto { pointer-events: auto; }
      .navigation-strip { position: absolute; inset-inline: 0; top: 0; height: 44px; transform: translateY(-100%); z-index: 90; }
      .thread-graph-composer-form { height: 60px; }
      .matter-workbench { position: relative; height: 700px; }
      #expand, #copy { position: absolute; }
    </style>
    <div class="thread-ui-shell"><div class="matter-workbench">
      <button id="expand" onclick="this.textContent='Expanded'">Click to expand</button>
      <button id="copy" onclick="this.textContent='Copied'">Copy reply</button>
      <div class="thread-graph-composer-host"><div class="thread-graph-composer-layer">
        <div class="navigation-strip pointer-events-none">
          <div class="thread-jump-latest-cluster pointer-events-none">
            <div role="group" aria-label="Timeline navigation" class="thread-jump-nav" data-following>
              <button class="thread-jump-turn" aria-label="Previous">↑</button><button class="thread-jump-turn" aria-label="Next">↓</button><button class="thread-jump-latest" data-action="jump-latest" onclick="this.textContent='Following'">Latest</button>
            </div>
          </div>
        </div>
        <form class="thread-graph-composer-form"><input aria-label="Prompt"></form>
      </div></div>
    </div></div>`);

    // Position the transcript control in the invisible strip, beside the visible capsule.
    await page.locator('#expand').evaluate(button => {
      const strip = document.querySelector('.navigation-strip')!.getBoundingClientRect();
      const parent = button.parentElement!.getBoundingClientRect();
      button.style.left = `${strip.left - parent.left + 4}px`;
      button.style.top = `${strip.top - parent.top + 8}px`;
    });
    await page.locator('#copy').evaluate(button => {
      const strip = document.querySelector('.navigation-strip')!.getBoundingClientRect();
      const parent = button.parentElement!.getBoundingClientRect();
      button.style.left = `${strip.left - parent.left + 120}px`;
      button.style.top = `${strip.top - parent.top + 8}px`;
    });
    await page.getByRole('button', { name: 'Copy reply', exact: true }).click({ timeout: 3000 });
    await expect(page.locator('#copy')).toHaveText('Copied');
    await page.getByRole('button', { name: 'Click to expand', exact: true }).click({ timeout: 3000 });
    await expect(page.locator('#expand')).toHaveText('Expanded');
    await page.getByRole('button', { name: 'Latest', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Following', exact: true })).toBeVisible();
    await page.getByRole('textbox', { name: 'Prompt' }).fill('Still editable');
    await expect(page.getByRole('textbox', { name: 'Prompt' })).toHaveValue('Still editable');
  }
});
