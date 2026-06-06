import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import { configDefaults, defineConfig } from 'vitest/config';

const quarryApiOrigin = process.env.QUARRY_API_ORIGIN ?? 'http://127.0.0.1:7831';

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 5173,
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
