import { test as base, expect } from 'playwright/test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

// Opt in when measuring normal-power rendering on a battery-powered runner.
// Chrome's Energy Saver can cap even a blank page at 30 fps. Change only the
// disposable benchmark profile, and report the setting with the measurements.
export const test = process.env.QUARRY_PERFORMANCE_NORMAL_POWER === '1' ? base.extend({
  browser: [async ({ playwright, browserName, headless, channel, launchOptions }, use) => {
    expect(browserName, 'Normal-power calibration currently supports Chrome').toBe('chromium');
    const profile = mkdtempSync(join(tmpdir(), 'quarry-performance-'));
    const context = await playwright.chromium.launchPersistentContext(profile, { ...launchOptions, headless, channel });
    try {
      const settings = await context.newPage();
      await settings.goto('chrome://settings/performance');
      const energySaver = await settings.evaluate(async () => {
        const api = (window as unknown as { chrome: { settingsPrivate: {
          getPref: (key: string, callback: (pref: { value: number }) => void) => void;
          setPref: (key: string, value: number, pageId: string, callback: (success: boolean) => void) => void;
        } } }).chrome.settingsPrivate;
        const key = 'performance_tuning.battery_saver_mode.state';
        const before = await new Promise<{ value: number }>((resolve) => api.getPref(key, resolve));
        const changed = await new Promise<boolean>((resolve) => api.setPref(key, 0, '', resolve));
        const after = await new Promise<{ value: number }>((resolve) => api.getPref(key, resolve));
        return { before: before.value, after: after.value, changed };
      });
      expect(energySaver.changed).toBe(true); expect(energySaver.after).toBe(0);
      console.info('QUARRY_PERFORMANCE_ENVIRONMENT', JSON.stringify({ energy_saver: energySaver, profile: 'disposable' }));
      await settings.close();
      const browser = context.browser();
      if (!browser) throw new Error('The benchmark browser is unavailable');
      await use(browser);
    } finally { await context.close(); rmSync(profile, { recursive: true, force: true }); }
  }, { scope: 'worker' }],
}) : base;
