import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { defineConfig, devices } from 'playwright/test';

const configDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(configDir, '..');
const liveRoot = mkdtempSync(join(tmpdir(), 'quarry-live-'));
const apiPort = process.env.QUARRY_LIVE_API_PORT ?? '7832';
const productionUi = process.env.QUARRY_PRODUCTION_UI === '1';
const uiPort = productionUi ? apiPort : process.env.QUARRY_LIVE_UI_PORT ?? '5174';
const apiOrigin = `http://127.0.0.1:${apiPort}`;
const uiOrigin = `http://127.0.0.1:${uiPort}`;

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
    { name: 'chromium', use: { ...devices['Desktop Chrome'], ...(process.env.QUARRY_SYSTEM_CHROME === '1' ? { channel: 'chrome' } : {}) } },
    { name: 'firefox', use: { ...devices['Desktop Firefox'] } },
    { name: 'webkit', use: { ...devices['Desktop Safari'] } },
  ],
  testDir: './tests',
  testMatch: process.env.QUARRY_PERFORMANCE ? /live-performance\.spec\.ts/ : /live-(?!performance).*\.spec\.ts/,
  use: {
    baseURL: uiOrigin,
    trace: 'retain-on-failure',
  },
  webServer: [
    {
      // The live suite drives the library surface (`/v1/libraries`, Git
      // sync-backed documents), which is feature-gated off in default builds.
      command: `cargo run ${process.env.QUARRY_PERFORMANCE || productionUi ? '--release ' : ''}-p quarry --features lib-documents -- server start --root "${liveRoot}" --addr 127.0.0.1:${apiPort}`,
      cwd: repoRoot,
      env: { ...inheritedEnv(), RUST_LOG: 'quarry_server=warn' },
      gracefulShutdown: { signal: 'SIGTERM', timeout: 2_000 },
      name: 'quarry',
      reuseExistingServer: false,
      timeout: 120_000,
      url: `${apiOrigin}/v1/libraries`,
    },
    ...(productionUi ? [] : [{
      command: `bun run document:build && bunx vite --host 127.0.0.1 --port ${uiPort}`,
      cwd: configDir,
      env: { ...inheritedEnv(), QUARRY_API_ORIGIN: apiOrigin },
      gracefulShutdown: { signal: 'SIGTERM', timeout: 2_000 },
      name: 'vite',
      reuseExistingServer: false,
      timeout: 120_000,
      url: uiOrigin,
    }]),
  ],
});
