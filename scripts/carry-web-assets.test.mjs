import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createServer } from 'node:http';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
const exec = promisify(execFile);

test('bounded concurrent carry preserves dependency traversal, expiry and current assets', async () => {
  const root = await mkdtemp(join(tmpdir(), 'carry-assets-test-'));
  await mkdir(join(root, 'assets'));
  await writeFile(join(root, 'assets', 'current.js'), 'current');
  const now = Date.now();
  const old = Array.from({length: 8}, (_, i) => `old-${i}.js`);
  let active = 0, maximum = 0;
  const requests = [];
  const server = createServer((req, res) => {
    requests.push(req.url);
    active += 1; maximum = Math.max(maximum, active);
    setTimeout(() => {
      active -= 1;
      if (req.url === '/') { res.setHeader('content-type', 'text/html'); res.end(old.map(name => `assets/${name}`).join(' ')); }
      else if (req.url === '/asset-history.json') { res.setHeader('content-type', 'application/json'); res.end(JSON.stringify({files:{'history.js':now-1000,'expired.js':now-100*3600000,'../private.js':now}})); }
      else if (req.url === '/assets/old-0.js') res.end('import "./child.css"');
      else res.end('retained');
    }, 15);
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try {
    const { stdout } = await exec(process.execPath, ['scripts/carry-web-assets.mjs', root, `http://127.0.0.1:${server.address().port}`], {timeout:5000});
    const history = JSON.parse(await readFile(join(root,'asset-history.json'),'utf8'));
    for (const name of [...old,'child.css','history.js','current.js']) assert.ok(history.files[name], name);
    assert.equal(history.files['expired.js'], undefined);
    assert.equal(requests.includes('/assets/expired.js'), false);
    assert.equal(await readFile(join(root,'assets/current.js'),'utf8'),'current');
    assert.ok(maximum > 1 && maximum <= 6, `parallel fetch count ${maximum}`);
    assert.equal(stdout.includes('127.0.0.1'),false,'logs must not expose the deployment origin');
  } finally {
    await new Promise(resolve => server.close(resolve));
    await rm(root,{recursive:true,force:true});
  }
});

test('carry retains newest-first byte budget despite response ordering', async () => {
  const root=await mkdtemp(join(tmpdir(),'carry-budget-test-')); await mkdir(join(root,'assets'));
  const now=Date.now();
  const server=createServer((req,res)=>{
    if(req.url==='/') { res.setHeader('content-type','text/html'); res.end(''); }
    else if(req.url==='/asset-history.json') { res.setHeader('content-type','application/json'); res.end(JSON.stringify({files:{'older.js':now-1000,'newer.js':now}})); }
    else setTimeout(()=>res.end('1234'),req.url.includes('newer')?30:1);
  });
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  try {
    await exec(process.execPath,['scripts/carry-web-assets.mjs',root,`http://127.0.0.1:${server.address().port}`],{timeout:5000,env:{...process.env,CARRY_MAX_MEGABYTES:String(4/1024/1024)}});
    const history=JSON.parse(await readFile(join(root,'asset-history.json'),'utf8'));
    assert.ok(history.files['newer.js']); assert.equal(history.files['older.js'],undefined);
  } finally { await new Promise(resolve=>server.close(resolve)); await rm(root,{recursive:true,force:true}); }
});
