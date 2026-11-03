import { test, expect, _electron as electron, type Page } from '@playwright/test';
import { fixture } from './helpers/account-key-fixture';
import { createRequire } from 'node:module';
import { resolve, join } from 'node:path';
import { writeFileSync } from 'node:fs';
async function inspect(page: Page, name: string) {
 const result=await page.evaluate(()=>{
  function describe(el: Element) {const r=el.getBoundingClientRect(),s=getComputedStyle(el);return {tag:el.tagName,cls:el.getAttribute('class'),label:el.getAttribute('aria-label'),rect:{x:r.x,y:r.y,w:r.width,h:r.height},position:s.position,display:s.display,visibility:s.visibility,top:s.top,left:s.left,transform:s.transform,offsetParent:(el as HTMLElement).offsetParent?.getAttribute('class')};}
  return [...document.querySelectorAll('svg')].filter(el=>{const r=el.getBoundingClientRect(),s=getComputedStyle(el);return r.x<55&&r.right>0&&r.bottom>0&&r.y<innerHeight&&r.width>0&&s.visibility!=='hidden'&&s.display!=='none';}).map(el=>({element:describe(el),ancestors:[el.parentElement,el.parentElement?.parentElement,el.parentElement?.parentElement?.parentElement,el.parentElement?.parentElement?.parentElement?.parentElement].filter((e):e is Element=>!!e).map(describe)}));
 });
 writeFileSync(`.scratch/account-shell/${name}-dom.json`,JSON.stringify(result,null,2));
 const menus=await page.locator('.matter-thread-tools-menu').evaluateAll(elements=>elements.map(el=>{const s=getComputedStyle(el),r=el.getBoundingClientRect();return {element:el.className,parent:el.parentElement?.className,hasShell:!!el.closest('.thread-ui-shell'),position:s.position,visibility:s.visibility,left:r.left,top:r.top};}));
 writeFileSync(`.scratch/account-shell/${name}-menus.json`,JSON.stringify(menus,null,2));
 await page.screenshot({path:`.scratch/account-shell/${name}.png`,mask:[page.locator('code'),page.locator('pre')]});
}
test('rail stays aligned and free of stray icons in real Electron and a wide browser', async ({browser},info)=>{
 const f=await fixture(browser,info);let app;
 const phase='corrected';
 try {
  const owner=await f.account('rail-review');
  const device=await f.device(owner,1,{PORT:'16461',POCKYMOE_RELAY_SUPERVISOR_PORT:'16461'});
  await owner.page.setViewportSize({width:1440,height:900});await owner.page.goto(device.url);
  await expect(owner.page.locator('.matter-rail')).toBeVisible();await inspect(owner.page,`rail-${phase}-browser-thread`);await assertClearRail(owner.page);
  const root=process.env.POCKYMOE_ELECTRON_APP_ROOT!;const req=createRequire(resolve(root,'package.json'));
  const env=Object.fromEntries(Object.entries(process.env).filter(([key])=>['PATH','HOME','TMPDIR','USER','LOGNAME'].includes(key))) as Record<string,string>;
  Object.assign(env,{POCKYMOE_DESKTOP_E2E:'1',POCKYMOE_DESKTOP_USER_DATA:join(f.root,'electron-profile'),POCKYMOE_DESKTOP_WEB_ORIGIN:f.base,POCKYMOE_DESKTOP_RELAY_ORIGIN:f.base});
  app=await electron.launch({executablePath:req('electron'),args:['--use-mock-keychain',resolve(root,'out/main/index.js')],env});
  await app.context().addInitScript(id=>{localStorage.setItem('remote-codex.locale','en');localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin,`relay:${id}`])}`,JSON.stringify({welcomeDismissed:true,completed:[],resume:{}}));},owner.userId);
  await expect.poll(()=>app!.windows().some(p=>p.url().startsWith(f.base))).toBe(true);const page=app.windows().find(p=>p.url().startsWith(f.base))!;
  await (await app.browserWindow(page)).evaluate(w=>w.setBounds({width:1440,height:900}));
  await page.goto(f.base+'/relay-portal?returnTo=%2Fdesktop%2Fdevices');
  await page.getByLabel('Email or username',{exact:true}).fill(owner.username);await page.getByLabel('Password',{exact:true}).fill(owner.password);await page.getByRole('button',{name:'Sign in',exact:true}).click();
  await expect(page.locator('.desktop-map-scene:not([hidden]) .matter-rail')).toBeVisible();await inspect(page,`rail-${phase}-electron-devices`);await assertClearRail(page);
  const go=(path:string)=>app!.evaluate((_e,path)=>{void (globalThis as any).__pockymoeDesktopTest.windows.dispatch({type:'navigate',path});},path);
  const positions: Record<string,unknown> = {};
  for (const pack of ['classic','plum-pocket','terminal']) for (const mode of ['light','dark']) {
   await page.evaluate(({pack,mode})=>{localStorage.setItem('pockymoe-theme-preset',pack);localStorage.setItem('remote-codex-theme-mode',mode);},{pack,mode});
   await page.reload();
   let reference: unknown;
   for(const [name,path] of [['devices','/desktop/devices'],['account','/relay-account'],['workspaces',`/devices/${device.id}/workspaces`],['thread',new URL(device.url).pathname]]) {
    await go(path);
    if(name==='devices') await expect(page.getByTestId('desktop-devices')).toBeVisible();
    else if(name==='thread') await expect(page.getByRole('textbox',{name:'Prompt',exact:true})).toBeVisible();
    else await expect(page.locator(`.desktop-route:not([hidden]) [data-page="${name}"]`)).toBeVisible();
    await expect(page.locator('[data-account-rail]:visible')).toBeVisible();
    await assertClearRail(page);
    const current=await railPositions(page);
    if(reference) expect(current, `${pack} ${mode} ${name}: global rail positions`).toEqual(reference); else reference=current;
    positions[`${pack}-${mode}-${name}`]=current;
    await inspect(page,`rail-${phase}-${pack}-${mode}-desktop-${name}`);
   }
   await page.locator('.desktop-route:not([hidden]) [data-account-rail]').click();
   await expect(page.getByRole('menu')).toBeVisible();
   await page.screenshot({path:`.scratch/account-shell/rail-${phase}-${pack}-${mode}-desktop-menu.png`,mask:[page.locator('code'),page.locator('pre')]});
   await page.keyboard.press('Escape');
   await owner.page.evaluate(({pack,mode})=>{localStorage.setItem('pockymoe-theme-preset',pack);localStorage.setItem('remote-codex-theme-mode',mode);},{pack,mode});
   await owner.page.reload();await expect(owner.page.getByRole('textbox',{name:'Prompt',exact:true})).toBeVisible();
   await assertClearRail(owner.page);await inspect(owner.page,`rail-${phase}-${pack}-${mode}-wide-browser`);
   await owner.page.setViewportSize({width:390,height:844});
   await inspect(owner.page,`rail-${phase}-${pack}-${mode}-phone-browser`);
   await owner.page.setViewportSize({width:1440,height:900});
  }
  writeFileSync('.scratch/account-shell/rail-corrected-positions.json',JSON.stringify(positions,null,2));

 } finally {if(app)await app.close();await f.close();}
});

async function assertClearRail(page: Page) {
 const leaks=await page.evaluate(()=>{
  const rails=[...document.querySelectorAll<HTMLElement>('.matter-rail')].filter(el=>el.getBoundingClientRect().width>0&&getComputedStyle(el).visibility!=='hidden');
  return rails.flatMap(rail=>{
   const rect=rail.getBoundingClientRect();
   return [...document.querySelectorAll('svg')].filter(el=>{
    const r=el.getBoundingClientRect(),style=getComputedStyle(el);
    return !rail.contains(el)&&r.width>0&&r.height>0&&style.visibility==='visible'&&style.display!=='none'&&r.left<rect.right&&r.right>rect.left&&r.top<rect.bottom&&r.bottom>rect.top;
   }).map(el=>({element:el.getAttribute('class'),parent:el.parentElement?.getAttribute('class')}));
  });
 });
 expect(leaks,'No painted SVG outside the rail may overlap its controls').toEqual([]);
 const nativeRail=page.locator('[data-desktop-surface] .matter-rail:visible');
 if(await nativeRail.count()) { await expect(nativeRail.locator('.device-monitor-button')).not.toBeVisible(); await expect(nativeRail.locator('.pm-tour-launcher')).not.toBeVisible(); }
 for (const menu of await page.locator('.matter-thread-tools-menu:not([data-open])').all()) { await expect(menu).toHaveCSS('visibility','hidden'); await expect(menu).toHaveCSS('position','fixed'); }
}
async function railPositions(page: Page) {
 return page.evaluate(()=>{
  const rail=[...document.querySelectorAll<HTMLElement>('.matter-rail')].find(el=>el.getBoundingClientRect().width>0&&getComputedStyle(el).visibility!=='hidden')!;
  const keys: Record<string,string>={logo:'.matter-brand',map:'[data-desktop-map-link]',devices:'button:has([data-icon=devices])',chat:'button:has([data-icon=chat])',explorer:'button:has([data-icon=explorer])',terminal:'button:has([data-icon=terminal])',avatar:'[data-account-rail]'};
  return Object.fromEntries(Object.entries(keys).map(([key,selector])=>{const element=rail.querySelector(selector);if(!element)throw Error('Missing rail item '+key);const r=element.getBoundingClientRect();return [key,{x:r.x,y:r.y,width:r.width,height:r.height}];}));
 });
}
