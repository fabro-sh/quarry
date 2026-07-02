import { defineConfig, devices } from 'playwright/test';

export default defineConfig({
  expect: {
    timeout: 5_000,
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
  // Shared CI runners are slow enough to trip tight focus/timing
  // assertions; retry there but keep local runs strict.
  retries: process.env.CI ? 2 : 0,
  testDir: './tests',
  // live-*.spec.ts run against a real quarry server via playwright.live.config.ts.
  testIgnore: /live-.*\.spec\.ts/,
  use: {
    baseURL: 'http://127.0.0.1:5173',
    trace: 'retain-on-failure',
  },
  webServer: {
    command: 'bun run dev -- --host 127.0.0.1 --port 5173',
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
    url: 'http://127.0.0.1:5173',
  },
});
