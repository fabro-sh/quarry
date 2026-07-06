import { test as base, expect, type Page } from 'playwright/test';

// Console errors the suite knowingly produces. Keep this list short, and give
// every entry a comment saying which flow emits it and why it is acceptable.
export const ALLOWED_CONSOLE_ERRORS: RegExp[] = [
  // The mock /v1 API answers unmocked endpoints with 404 by design; the
  // browser logs each as a network error. JS errors are never 404 logs, so
  // this cannot mask the uncaught-error class the guard exists for.
  /^Failed to load resource: the server responded with a status of 404 /,
];

// Every test fails if any page reported an uncaught error or logged an error
// to the console. Bugs whose only symptom is a console error nobody is
// required to look at — a web worker crashing on load, an unhandled promise
// rejection — stay invisible to assertions that only check the happy path.
// Uncaught errors inside web workers never fire `pageerror` (they surface
// only as console messages), so both channels are collected.
export const test = base.extend({
  context: async ({ context }, use) => {
    const errors: string[] = [];
    const watch = (page: Page) => {
      page.on('pageerror', (error) => {
        errors.push(`pageerror: ${error.message}`);
      });
      page.on('console', (message) => {
        if (message.type() !== 'error') return;
        const text = message.text();
        if (ALLOWED_CONSOLE_ERRORS.some((allowed) => allowed.test(text))) return;
        errors.push(`console.error: ${text}`);
      });
    };
    context.pages().forEach(watch);
    context.on('page', watch);
    await use(context);
    expect(errors, 'no page may report uncaught errors or console errors').toEqual([]);
  },
});

export { expect } from 'playwright/test';
