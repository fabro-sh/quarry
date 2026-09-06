import { expect, test } from 'playwright/test';
import { blocks, body, createDocument, openDocument, select, transaction } from './helpers/native-document';

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
    await user.page.route(/\/document-state(?:\?|$)/, async (route) => {
      if (!held) { held = true; await gate; }
      await route.continue().catch(() => {}); // The held request belongs to the page that was unloaded.
    });
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
      window.dispatchEvent(new Event('online'));
    });
    await expect.poll(() => held).toBe(true);
    if (restoreCachedPage) {
      await user.page.evaluate(() => window.dispatchEvent(new PageTransitionEvent('pagehide', { persisted: true })));
      expect(await user.page.evaluate(() => sessionStorage.getItem('test-read-events'))).toMatch(/^abort;/);
      const [current] = await blocks(request, fixture.url);
      await transaction(request, fixture.url, [{ op: 'replace_block_content', block_id: current.block_id, text: 'Agent ' + current.text }], current.document_clock);
      release();
      await user.page.evaluate(() => window.dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true })));
      await expect(body(user.page)).toHaveText('Agent Local TARGET');
    }
    await user.page.reload();
    release();
    await expect(body(user.page)).toHaveText(restoreCachedPage ? 'Agent Local TARGET' : 'Local TARGET');
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    const events = await user.page.evaluate(() => sessionStorage.getItem('test-read-events'));
    if (browserName === 'webkit' && !restoreCachedPage) {
      // WebKit can reject its network request before dispatching pagehide.
      // In that ordering there is no active read left for the app to abort.
      expect(events).toMatch(/^(?:abort;|TypeError: Load failed;pagehide;)/);
    } else expect(events).toMatch(/^abort;/);
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
