import { fileURLToPath } from 'node:url';

import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import { configDefaults, defineConfig } from 'vitest/config';

const quarryApiOrigin = process.env.QUARRY_API_ORIGIN ?? 'http://127.0.0.1:7831';

export default defineConfig({
  plugins: [react(), tailwindcss()],
  build: {
    // The server's CSP has no `data:` in font/style sources, so small assets
    // (like the 2KB cyrillic-ext font subset) must be emitted as files, not
    // inlined as data: URIs.
    assetsInlineLimit: 0,
  },
  resolve: {
    alias: {
      // Vite resolves the `browser` export condition even for worker bundles,
      // and this package's browser build touches `document` at module scope —
      // crashing the mirror-serializer worker on load. Its default build works
      // in both window and worker contexts.
      'decode-named-character-reference': fileURLToPath(
        new URL('./node_modules/decode-named-character-reference/index.js', import.meta.url)
      ),
    },
  },
  server: {
    port: 5173,
    // Tailscale serve/funnel fronts the dev server under the tailnet
    // hostname; the leading dot allows any device on this tailnet.
    allowedHosts: ['.manatee-truck.ts.net', '.walleye-rainbow.ts.net'],
    proxy: {
      '/v1': {
        target: quarryApiOrigin,
        ws: true,
      },
      '/quarry.SKILL.md': {
        target: quarryApiOrigin,
      },
      '/agent-docs': {
        target: quarryApiOrigin,
      },
      '/.well-known/agent.json': {
        target: quarryApiOrigin,
      },
    },
  },
  test: {
    environment: 'jsdom',
    environmentOptions: {
      jsdom: {
        url: 'http://127.0.0.1/',
      },
    },
    exclude: [...configDefaults.exclude, 'tests/**'],
    globals: true,
    setupFiles: './vitest.setup.ts',
  },
});
