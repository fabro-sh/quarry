import { createServer } from 'node:http';
import { expect, test } from 'playwright/test';
import { blocks, body, createDocument, openDocument, select, transaction, uiOrigin } from './helpers/native-document';

for (const restoreCachedPage of [false, true]) test(`navigation interrupts a pending read and preserves text and comments (cached restore: ${restoreCachedPage})`, async ({ browser, browserName, request }) => {
  const fixture = await createDocument(request, 'TARGET');
  const [target] = await blocks(request, fixture.url);
  await transaction(request, fixture.url, [{ op: 'comment.add', block_id: target.block_id, start: 0, end: 6, body: 'Exact characters' }], target.document_clock);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  let held = false;
  try {
    await select(user.page, target.block_id, 0);
    await user.page.keyboard.type('Local ');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.evaluate(() => {
      const record = (event: string) => sessionStorage.setItem('test-read-events', (sessionStorage.getItem('test-read-events') ?? '') + event + ';');
      window.addEventListener('pagehide', () => record('pagehide'));
      const fetch = window.fetch.bind(window);
      window.fetch = (input, options) => {
        if (String(input).includes('/document-state')) {
          options?.signal?.addEventListener('abort', () => record('abort'), { once: true });
        }
        return fetch(input, options).catch((error) => { record(String(error)); throw error; });
      };
    });
    // Install the signal observer before holding a read. A background refresh
    // can start while route registration awaits the browser protocol.
    await user.page.route(/\/document-state(?:\?|$)/, async (route) => {
      if (!held) { held = true; await gate; }
      await route.continue().catch(() => {}); // The held request belongs to the page that was unloaded.
    });
    await user.page.evaluate(() => window.dispatchEvent(new Event('online')));
    await expect.poll(() => held).toBe(true);
    if (restoreCachedPage) {
      await user.page.evaluate(() => {
        sessionStorage.setItem('test-read-events', 'cached-hide;');
        window.dispatchEvent(new PageTransitionEvent('pagehide', { persisted: true }));
      });
      expect(await user.page.evaluate(() => sessionStorage.getItem('test-read-events'))).toMatch(/^cached-hide;abort;/);
      const [current] = await blocks(request, fixture.url);
      await transaction(request, fixture.url, [{ op: 'replace_block_content', block_id: current.block_id, text: 'Agent ' + current.text }], current.document_clock);
      release();
      await user.page.evaluate(() => window.dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true })));
      await expect(body(user.page)).toHaveText('Agent Local TARGET');
    }
    await user.page.evaluate(() => sessionStorage.setItem('test-read-events', 'reload;'));
    await user.page.reload();
    release();
    await expect(body(user.page)).toHaveText(restoreCachedPage ? 'Agent Local TARGET' : 'Local TARGET');
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    const recordedEvents = await user.page.evaluate(() => sessionStorage.getItem('test-read-events'));
    expect(recordedEvents).toMatch(/^reload;/);
    const events = recordedEvents!.slice('reload;'.length);
    // A real navigation can tear down the JavaScript realm immediately
    // after pagehide. The browser still cancels the held fetch, but its
    // AbortSignal callback is not guaranteed to update sessionStorage.
    expect(events).toMatch(/^(?:abort;|pagehide;|TypeError: (?:Load failed|Failed to fetch|NetworkError when attempting to fetch resource\.);(?:abort;|pagehide;))/);
    expect(events).toContain('pagehide;');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    expect(user.errors).toEqual([]);
  } finally { release(); await user.context.close(); }
});

test('browser error collection retains access errors on an active page and application errors during navigation', async ({ browser, browserName, request }) => {
  test.skip(browserName !== 'webkit', 'Only WebKit uses the navigation diagnostic exception');
  const fixture = await createDocument(request, 'TARGET');
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await user.page.evaluate(() => {
      const editor = document.querySelector('[aria-label="Plate markdown editor"]')!;
      // An active-page access error must never be hidden by the reload exception.
      editor.addEventListener('test-error', () => {
        const message = `Fetch API cannot load ${location.origin}/v1/libraries/test/documents-by-id/test/document-state due to access control checks.`;
        const error = new Error(message); error.stack = message; throw error;
      }, { once: true });
      editor.dispatchEvent(new Event('test-error'));
    });
    await expect.poll(() => user.errors.length).toBe(1);
    await user.page.evaluate(() => {
      window.addEventListener('pagehide', () => { throw new Error('Application teardown failed'); }, { once: true });
    });
    await user.page.reload(); await expect(body(user.page)).toHaveText('TARGET');
    await expect.poll(() => user.errors.length).toBe(2);
    expect(user.errors[0]).toContain('due to access control checks.');
    expect(user.errors[1]).toBe('Application teardown failed');
  } finally { await user.context.close(); }
});


test('an unhandled cross-origin fetch remains visible to browser error collection', async ({ browser, request }) => {
  let requests = 0;
  const server = createServer((_request, response) => {
    requests++;
    response.writeHead(200, { 'Content-Type': 'application/json' });
    response.end('{}');
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  try {
    const address = server.address();
    if (!address || typeof address === 'string') throw new Error('Missing test server address');
    const fixture = await createDocument(request, 'TARGET');
    const user = await openDocument(browser, fixture.path, 'Reviewer');
    try {
      // Same endpoint shape, different origin, and deliberately no CORS header.
      // Returning the fetch from evaluate would catch its rejection in Node;
      // leave it unhandled in the page to exercise the actual error collector.
      await user.page.evaluate((url) => { void fetch(url); },
        `http://127.0.0.1:${address.port}/v1/libraries/test/documents-by-id/test/document-state`);
      await expect.poll(() => requests).toBe(1);
      await expect.poll(() => user.errors.length).toBeGreaterThan(0);
      expect(user.errors.join(' ')).toMatch(/fetch|load|access control|cross-origin|cors/i);
    } finally { await user.context.close(); }
  } finally {
    server.closeAllConnections();
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
});

test('an unhandled same-URL fetch cannot reuse a previous cancellation proof', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'TARGET');
  const documentId = (await request.get(fixture.url)).headers()['x-quarry-document-id'];
  expect(documentId).toBeTruthy();
  const url = `${uiOrigin}/v1/libraries/${fixture.library}/documents-by-id/${documentId}/document-state?error-collector-test=1`;
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await user.page.route(url, (route) => route.abort('aborted'));
    await Promise.all([
      user.page.waitForEvent('requestfailed', (failed) => failed.url() === url),
      user.page.evaluate(async (target) => { await fetch(target).catch(() => {}); }, url),
    ]);
    await Promise.all([
      user.page.waitForEvent('requestfailed', (failed) => failed.url() === url),
      user.page.evaluate((target) => { void fetch(target); }, url),
    ]);
    // The first caller handles its rejection. Only the second caller can
    // produce this unhandled TypeError; both use the identical request URL.
    await expect.poll(() => user.errors.some((error) => /(?:TypeError: )?(?:Load failed|Failed to fetch|NetworkError)/.test(error))).toBe(true);
  } finally { await user.context.close(); }
});
