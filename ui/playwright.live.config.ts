import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { defineConfig, devices } from 'playwright/test';

const configDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(configDir, '..');
const liveRoot = mkdtempSync(join(tmpdir(), 'quarry-live-'));
const apiOrigin = 'http://127.0.0.1:7832';
const uiOrigin = 'http://127.0.0.1:5174';

function inheritedEnv() {
  const env: Record<string, string> = {};
  for (const [key, value] of Object.entries(process.env)) {
    if (value !== undefined) env[key] = value;
  }
  return env;
}

export default defineConfig({
  expect: {
    timeout: 10_000,
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
  testDir: './tests',
  testMatch: /live-.*\.spec\.ts/,
  use: {
    baseURL: uiOrigin,
    trace: 'retain-on-failure',
  },
  webServer: [
    {
      command: `cargo run -p quarry -- --root "${liveRoot}" serve --addr 127.0.0.1:7832`,
      cwd: repoRoot,
      gracefulShutdown: { signal: 'SIGTERM', timeout: 2_000 },
      name: 'quarry',
      reuseExistingServer: false,
      timeout: 120_000,
      url: `${apiOrigin}/v1/libraries`,
    },
    {
      command: 'bun run dev -- --host 127.0.0.1 --port 5174',
      cwd: configDir,
      env: { ...inheritedEnv(), QUARRY_API_ORIGIN: apiOrigin },
      gracefulShutdown: { signal: 'SIGTERM', timeout: 2_000 },
      name: 'vite',
      reuseExistingServer: false,
      timeout: 60_000,
      url: uiOrigin,
    },
  ],
});
