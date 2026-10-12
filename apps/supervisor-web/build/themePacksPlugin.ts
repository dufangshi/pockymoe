/* Publishes the built-in theme packs in `themes/<id>/` (repo root) as static files:

   /themes/index.json        ids, versions, hashes and file lists
   /themes/<id>.css          the compiled stylesheet (asset URLs point below)
   /themes/<id>/<path>       the pack's own files, for assets and Export

   The app imports `virtual:pockymoe-theme-packs` (manifests, hashes, file lists),
   so a Classic user downloads no theme CSS at all. URLs carry `?v=<hash>`
   because the relay serves non-HTML files as immutable. The dev server serves
   the same paths from the source folders and reloads when they change. */

import { readFileSync, readdirSync, statSync } from 'node:fs';
import path from 'node:path';
import type { Plugin } from 'vite';

import { packFileType } from '../src/lib/themePacks/limits';
import { compileThemePack, packHash, validatePackFiles } from '../src/lib/themePacks/pack';
import type { BuiltinThemePack } from '../src/lib/themePacks/types';

const VIRTUAL_ID = 'virtual:pockymoe-theme-packs';
const RESOLVED_ID = `\0${VIRTUAL_ID}`;

interface LoadedPack extends BuiltinThemePack {
  sources: Map<string, Uint8Array>;
  compiled: string;
}

function readTree(dir: string, prefix = ''): [string, Uint8Array][] {
  return readdirSync(dir)
    .sort()
    .flatMap((name) => {
      const full = path.join(dir, name);
      if (statSync(full).isDirectory()) return readTree(full, `${prefix}${name}/`);
      return [[`${prefix}${name}`, new Uint8Array(readFileSync(full))] as [string, Uint8Array]];
    });
}

export async function loadBuiltinThemePacks(themesDir: string, base = '/'): Promise<LoadedPack[]> {
  const packs: LoadedPack[] = [];
  let entries: string[] = [];
  try {
    entries = readdirSync(themesDir).sort();
  } catch {
    return packs;
  }
  for (const folder of entries) {
    const dir = path.join(themesDir, folder);
    if (!statSync(dir).isDirectory()) continue;
    const pack = validatePackFiles(readTree(dir));
    if (pack.manifest.id !== folder) throw new Error(`themes/${folder}: theme.json id "${pack.manifest.id}" must match its folder`);
    const hash = await packHash(pack.files);
    const root = `${base}themes/${folder}/`;
    const compiled = compileThemePack(pack, (file) => `${root}${file}?v=${hash}`);
    packs.push({
      id: folder,
      hash,
      manifest: pack.manifest,
      files: [...pack.files.keys()],
      base: root,
      css: `${base}themes/${folder}.css?v=${hash}`,
      sources: pack.files,
      compiled,
    });
  }
  return packs;
}

function publicIndex(packs: LoadedPack[]) {
  return JSON.stringify({ schema: 1, packs: packs.map(({ id, hash, manifest, files }) => ({ id, version: manifest.version, hash, files })) }, null, 2);
}

export function themePacksPlugin({ themesDir }: { themesDir: string }): Plugin {
  let base = '/';
  let packs: Promise<LoadedPack[]> | null = null;
  const load = () => (packs ??= loadBuiltinThemePacks(themesDir, base));

  return {
    name: 'pockymoe-theme-packs',
    configResolved(config) {
      base = config.base;
    },
    resolveId(id) {
      return id === VIRTUAL_ID ? RESOLVED_ID : null;
    },
    async load(id) {
      if (id !== RESOLVED_ID) return null;
      const list: BuiltinThemePack[] = (await load()).map(({ id: packId, hash, manifest, files, base: root, css }) => ({ id: packId, hash, manifest, files, base: root, css }));
      return `export default ${JSON.stringify(list)};`;
    },
    configureServer(server) {
      server.watcher.add(themesDir);
      const changed = (file: string) => {
        if (!file.startsWith(themesDir + path.sep)) return;
        packs = null;
        const module = server.moduleGraph.getModuleById(RESOLVED_ID);
        if (module) server.moduleGraph.invalidateModule(module);
        server.ws.send({ type: 'full-reload' });
      };
      server.watcher.on('change', changed).on('add', changed).on('unlink', changed);
      server.middlewares.use(async (req, res, next) => {
        const url = (req.url ?? '').split('?')[0]!;
        const prefix = `${base}themes/`;
        if (!url.startsWith(prefix)) return next();
        const rest = decodeURIComponent(url.slice(prefix.length));
        let loaded: LoadedPack[];
        try {
          loaded = await load();
        } catch (error) {
          packs = null;
          server.config.logger.error(`[theme packs] ${(error as Error).message}`);
          res.statusCode = 500;
          return res.end(String((error as Error).message));
        }
        const send = (type: string, body: string | Uint8Array) => {
          res.setHeader('content-type', type);
          res.setHeader('cache-control', 'no-cache');
          res.end(body);
        };
        if (rest === 'index.json') return send('application/json; charset=utf-8', publicIndex(loaded));
        const css = /^([a-z0-9-]+)\.css$/.exec(rest);
        const cssPack = css && loaded.find((pack) => pack.id === css[1]);
        if (cssPack) return send('text/css; charset=utf-8', cssPack.compiled);
        const slash = rest.indexOf('/');
        const pack = slash > 0 ? loaded.find((entry) => entry.id === rest.slice(0, slash)) : undefined;
        const file = pack?.sources.get(rest.slice(slash + 1));
        if (pack && file) return send(packFileType(rest), file);
        res.statusCode = 404;
        res.end('Not found');
      });
    },
    async generateBundle() {
      const loaded = await load();
      this.emitFile({ type: 'asset', fileName: 'themes/index.json', source: publicIndex(loaded) });
      for (const pack of loaded) {
        this.emitFile({ type: 'asset', fileName: `themes/${pack.id}.css`, source: pack.compiled });
        for (const [file, bytes] of pack.sources) this.emitFile({ type: 'asset', fileName: `themes/${pack.id}/${file}`, source: bytes });
      }
    },
  };
}
