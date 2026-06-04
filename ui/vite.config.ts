import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import { configDefaults, defineConfig } from 'vitest/config';

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 5173,
    proxy: {
      '/v1': {
        target: 'http://127.0.0.1:7831',
        ws: true,
      },
      '/quarry.SKILL.md': {
        target: 'http://127.0.0.1:7831',
      },
      '/agent-docs': {
        target: 'http://127.0.0.1:7831',
      },
      '/.well-known/agent.json': {
        target: 'http://127.0.0.1:7831',
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
