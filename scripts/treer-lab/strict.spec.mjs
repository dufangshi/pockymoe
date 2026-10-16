import{createRequire}from'node:module';import{readFile,writeFile}from'node:fs/promises';
import{treerSource,root,origin,sessionCookie,read,api}from'./common.mjs';
const{test,expect}=createRequire(`${treerSource}/web/package.json`)('@playwright/test');
const dir=process.env.TREER_LAB_STRICT_EVIDENCE_DIR??`${root}/.scratch/treer-strict-l2`;
const f=JSON.parse(await readFile(`${dir}/fixture.json`)),s=await read(),ws=`/api/workspaces/${s.workspaceId}`;
const app=(await api(`${ws}/apps`)).apps.find(a=>a.name==='pockymoe-lab-hub');
async function auth(context){const cookie=(await sessionCookie()).split('; ').find(x=>x.startsWith('treer_session='));expect(Boolean(cookie)).toBe(true);await context.addCookies([{name:'treer_session',value:cookie.slice(14),url:origin,httpOnly:true,sameSite:'Lax'}]);}
test('strict child has its own tunnel and groups under parent; App exposes only metadata',async({page,context})=>{
 const parent=`${origin}${ws}/agents/${f.parentAgent}/interface/ui/`,child=`${origin}${ws}/agents/${f.childAgent}/interface/ui/`;
 expect((await context.request.get(parent,{maxRedirects:0})).status()).toBe(401);await auth(context);
 await page.goto(parent);await expect(page.locator('[contenteditable=true], [contenteditable=plaintext-only]').first()).toBeVisible();
 const group=page.locator('.matter-thread-group').filter({has:page.locator(`a[href$="/threads/${f.parentThread}"]`)});
 await expect(group.locator('.matter-group-toggle')).toHaveText(/1/);await group.locator('.matter-group-toggle').click();
 const menu=page.locator('#matter-agent-threads');await expect(menu.locator(`a[href$="/threads/${f.childThread}"]`)).toBeVisible();
 await menu.locator(`a[href$="/threads/${f.childThread}"]`).click();await expect(page).toHaveURL(`${parent}threads/${f.childThread}`);
 const r=await context.request.get(child,{maxRedirects:0});expect(r.status()).toBe(307);expect(r.headers().location).toBe(`threads/${f.childThread}`);
 await page.goto(`${origin}${ws}/apps/${app.app_id}/ui/`);await expect(page.getByRole('heading',{name:'Pockymoe hub'})).toBeVisible();
 await expect(page.locator('body')).toContainText('Use a Pockymoe Agent to open conversations');await expect(page.locator('body')).not.toContainText(f.parentName);
 expect((await context.request.get(`${origin}${ws}/apps/${app.app_id}/ui/api/threads`)).status()).toBe(403);
 await writeFile(`${dir}/browser.json`,JSON.stringify({result:'PASS',project:'desktop-chromium',spec:'strict.spec.mjs',childThread:f.childThread,appReadOnly:true},null,2));
});
