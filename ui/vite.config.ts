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
  server: {
    port: 5173,
    // The browser and Rust codec consume the same block capability manifest.
    // It lives one level above the UI package so Cargo includes it when the
    // codec is packaged; allow Vite's dev server to serve that shared file.
    fs: {
      allow: [fileURLToPath(new URL('..', import.meta.url))],
    },
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
