// Keeps the previous builds' hashed assets in a new Pages build.
//
// Each deployment replaces the hashed chunks, and a page opened before it loads
// its lazy chunks (Files, the editor, highlighting) from the build it started
// with. Without them it can only reload. This copies the assets the live site
// serves now, and those it carried itself, into the new build, for MAX_AGE_HOURS
// after they stopped being current and up to MAX_MEGABYTES in all, newest first.
// `asset-history.json` at the site root records when each asset was last current.
//
// usage: node scripts/carry-web-assets.mjs <dist> <live origin>
import { mkdir, readFile, readdir, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';

const MAX_AGE_HOURS = Number(process.env.CARRY_MAX_AGE_HOURS ?? 72);
const MAX_MEGABYTES = Number(process.env.CARRY_MAX_MEGABYTES ?? 300);
const HISTORY = 'asset-history.json';
const NAME = /^[A-Za-z0-9_.-]+\.(?:js|css|woff2?|ttf|svg|png|webp)$/;
// References between Vite's assets: "assets/x.js" in the entry and in preload
// maps, "./x.js" between chunks, url(x.woff2) in styles.
const REFERENCE = /(?:assets\/|\.\/|url\()([A-Za-z0-9_.-]+\.(?:js|css|woff2?|ttf|svg|png|webp))/g;
// The page itself names only its entry assets, under assets/.
const PAGE_REFERENCE = /assets\/([A-Za-z0-9_.-]+\.(?:js|css))/g;

const [dist, origin] = process.argv.slice(2);
if (!dist || !origin) throw new Error('usage: carry-web-assets.mjs <dist> <live origin>');
const assetsDir = path.join(dist, 'assets');
const now = Date.now();

async function fetchBytes(url) {
  for (let attempt = 0; attempt < 3; attempt += 1) {
    try {
      const response = await fetch(url, { redirect: 'follow', signal: AbortSignal.timeout(20_000) });
      if (response.status === 404) return null;
      // A host that answers a missing asset with the app page has no such asset.
      if (response.ok && /text\/html/.test(response.headers.get('content-type') ?? '') && !String(url).endsWith('/')) return null;
      if (response.ok) return new Uint8Array(await response.arrayBuffer());
    } catch { /* try again */ }
  }
  return null;
}

const current = new Set((await readdir(assetsDir, { withFileTypes: true })).filter(entry => entry.isFile()).map(entry => entry.name));

// What the live site carried, with when each was last current.
const history = new Map();
const previous = await fetchBytes(new URL(`/${HISTORY}`, origin));
if (previous) {
  try {
    for (const [name, at] of Object.entries(JSON.parse(new TextDecoder().decode(previous)).files ?? {})) {
      if (NAME.test(name) && Number.isFinite(at)) history.set(name, at);
    }
  } catch { /* a broken history only loses the older generations */ }
}

// The live build itself, found from its entry: it stops being current now.
const live = new Map();
const queue = [];
const html = await fetchBytes(new URL('/', origin));
if (html) for (const match of new TextDecoder().decode(html).matchAll(PAGE_REFERENCE)) queue.push(match[1]);
while (queue.length) {
  const name = queue.shift();
  if (live.has(name) || !NAME.test(name)) continue;
  const bytes = await fetchBytes(new URL(`/assets/${name}`, origin));
  live.set(name, bytes);
  if (!bytes || !/\.(?:js|css)$/.test(name)) continue;
  for (const match of new TextDecoder().decode(bytes).matchAll(REFERENCE)) if (!live.has(match[1])) queue.push(match[1]);
}
for (const [name, bytes] of live) if (bytes) history.set(name, now);

// Newest first, within the age and size limits.
const oldest = now - MAX_AGE_HOURS * 3_600_000;
let budget = MAX_MEGABYTES * 1024 * 1024;
for (const name of current) budget -= (await stat(path.join(assetsDir, name))).size;
const carried = {};
let count = 0;
for (const [name, at] of [...history].sort((a, b) => b[1] - a[1])) {
  if (current.has(name) || at < oldest) continue;
  const bytes = live.get(name) ?? await fetchBytes(new URL(`/assets/${name}`, origin));
  if (!bytes || bytes.byteLength > budget) continue;
  budget -= bytes.byteLength;
  await mkdir(assetsDir, { recursive: true });
  await writeFile(path.join(assetsDir, name), bytes);
  carried[name] = at;
  count += 1;
}

const files = { ...carried };
for (const name of current) files[name] = now;
await writeFile(path.join(dist, HISTORY), `${JSON.stringify({ version: 1, files })}\n`);
const used = MAX_MEGABYTES - budget / 1024 / 1024;
console.log(`carried ${count} earlier assets (${current.size} current, ${used.toFixed(1)} MB in all) from ${origin}`);
