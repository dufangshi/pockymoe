import path from 'node:path';
import { createRequire } from 'node:module';
import { copyFileSync, mkdirSync } from 'node:fs';

import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import { defineConfig, loadEnv } from 'vite';
import { staticHosting } from './staticHosting';

const require = createRequire(import.meta.url);

function packageChunk(id: string) {
  // A stylesheet assigned to a vendor chunk makes that whole chunk a static
  // dependency of its importer (the shared UI imports xterm.css eagerly), so
  // leave CSS to the default chunking.
  if (!id.includes('/node_modules/') || /\.css($|\?)/.test(id)) {
    return null;
  }

  if (
    id.includes('/node_modules/react/') ||
    id.includes('/node_modules/react-dom/') ||
    id.includes('/node_modules/react-router-dom/')
  ) {
    return 'react-vendor';
  }

  if (
    id.includes('/node_modules/xterm/') ||
    id.includes('/node_modules/@xterm/')
  ) {
    return 'terminal-vendor';
  }

  if (
    id.includes('/node_modules/react-markdown/') ||
    id.includes('/node_modules/remark-') ||
    id.includes('/node_modules/rehype-') ||
    id.includes('/node_modules/micromark') ||
    id.includes('/node_modules/unified/') ||
    id.includes('/node_modules/katex/')
  ) {
    return 'markdown-vendor';
  }

  if (
    id.includes('/node_modules/react-syntax-highlighter/') ||
    id.includes('/node_modules/refractor/') ||
    id.includes('/node_modules/prismjs/')
  ) {
    return 'syntax-vendor';
  }

  if (
    id.includes('/node_modules/@xyflow/') ||
    id.includes('/node_modules/d3-')
  ) {
    return 'graph-vendor';
  }

  if (
    id.includes('/node_modules/@radix-ui/') ||
    id.includes('/node_modules/lucide-react/')
  ) {
    return 'ui-vendor';
  }

  return null;
}

export default defineConfig(({ mode }) => {
  if (process.env.VITEST || mode === 'test') {
    process.env.NODE_ENV = 'test';
  }

  const env = loadEnv(mode, __dirname, 'VITE_');
  const staticWeb = mode === 'static';
  const apiProxyTarget = process.env.VITE_API_PROXY_TARGET ?? 'http://127.0.0.1:8787';
  const wsProxyTarget = process.env.VITE_WS_PROXY_TARGET ?? 'ws://127.0.0.1:8787';
  const threadUiRoot = path.resolve(__dirname, '../../pockymoe-thread-ui');
  // One lucide-react copy for the app and the shared UI. Use its ES module
  // build: the CommonJS entry cannot be tree-shaken and ships every icon.
  const lucideReactPackage = require.resolve('lucide-react/package.json', { paths: [__dirname] });
  const lucideReactEntry = path.join(
    path.dirname(lucideReactPackage),
    (require(lucideReactPackage) as { module: string }).module,
  );
  const threeDmolEntry = require.resolve('3dmol', { paths: [__dirname] });
  const threeDmolSource = require.resolve('3dmol/build/3Dmol-min.js', {
    paths: [__dirname],
  });

  return {
    worker: {
      format: 'es',
      ...(staticWeb ? { rollupOptions: { output: {
        entryFileNames: (chunk: { name: string }) => chunk.name === 'relayServiceWorker'
          ? 'relayServiceWorker.js' : 'assets/[name]-[hash].js',
      } } } : {}),
    },
    define: staticWeb ? { 'import.meta.env.VITE_POCKYMOE_STATIC_WEB': JSON.stringify('true') } : {},
    plugins: [
      ...(staticWeb ? [staticHosting(
        env.VITE_POCKYMOE_RELAY_ORIGIN ?? process.env.VITE_POCKYMOE_RELAY_ORIGIN ?? '',
        process.env.POCKYMOE_WEB_DOMAIN ?? '',
      )] : []),
      react(),
      tailwindcss(),
      {
        name: 'copy-3dmol-runtime',
        apply: 'build',
        writeBundle(options) {
          const outputDir =
            typeof options.dir === 'string'
              ? options.dir
              : path.resolve(__dirname, 'dist');
          const vendorDir = path.join(outputDir, 'vendor');
          mkdirSync(vendorDir, { recursive: true });
          copyFileSync(threeDmolSource, path.join(vendorDir, '3Dmol-min.js'));
          if (staticWeb) {
            copyFileSync(path.resolve(__dirname, '../../scripts/setup.sh'), path.join(outputDir, 'setup.sh'));
            copyFileSync(path.resolve(__dirname, '../../scripts/setup.ps1'), path.join(outputDir, 'setup.ps1'));
          }
        },
      },
    ],
    resolve: {
      alias: {
        '3dmol': threeDmolEntry,
        'lucide-react': lucideReactEntry,
      },
    },
    optimizeDeps: {
      include: [
        'xterm',
        '@xterm/addon-fit',
        '@pockymoe/thread-ui',
        'use-sync-external-store/shim/with-selector',
      ],
    },
    server: {
      host: '0.0.0.0',
      allowedHosts: [
        'fonshs-macbook-pro.tailaf4fa.ts.net',
        'debug.lnz-study.com',
      ],
      fs: {
        allow: [
          path.resolve(__dirname, '../..'),
          threadUiRoot,
        ]
      },
      proxy: {
        '/api': apiProxyTarget,
        '/healthz': apiProxyTarget,
        '/ws': {
          target: wsProxyTarget,
          ws: true
        }
      }
    },
    preview: {
      host: '0.0.0.0'
    },
    test: {
      environment: 'jsdom',
      setupFiles: './src/test/setup.ts',
      server: {
        deps: {
          inline: [
            '@pockymoe/thread-ui',
            '@pockymoe/plugin-runtime',
            '@pockymoe/plugin-terminal',
          ],
        },
      },
      env: {
        NODE_ENV: 'test',
      },
    },
    build: {
      chunkSizeWarningLimit: 550,
      rollupOptions: {
        output: {
          // The shared UI is left to the default chunking so its lazy imports
          // (Monaco editor and diff, workspace panel, graph view, xterm, mermaid)
          // stay separate chunks instead of joining the first-paint bundle.
          manualChunks(id) {
            return packageChunk(id);
          },
        },
      },
    },
  };
});
