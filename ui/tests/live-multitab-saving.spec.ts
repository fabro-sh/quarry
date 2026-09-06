import { expect, test, type Page } from 'playwright/test';
import { apiOrigin, blocks, body, createDocument, review, select, transaction, uiOrigin } from './helpers/native-document';

test.describe.configure({ timeout: 90000 });

for (const scope of ['library', 'temporary'] as const) {
  test(`${scope}: eight tabs share one connection pool while browsers save and an agent comments`, async ({ browser, request }) => {
    const markdown = 'Before TARGET after.\n\nSecond paragraph.';
    const fixture = scope === 'library' ? await createDocument(request, markdown) : await (async () => {
      const response = await request.post(`${apiOrigin}/v1/tmp/documents`, { data: { content: markdown } });
      expect(response.ok()).toBe(true);
      const { document } = await response.json();
      return { url: `${apiOrigin}/v1/tmp/documents/${document.path}`, path: `/tmp/${document.path}` };
    })();
    const [first, second] = await blocks(request, fixture.url);
    // Separate browser contexts hide Chrome's per-origin HTTP/1 connection limit.
    const context = await browser.newContext();
    await context.addInitScript(() => localStorage.setItem('quarry:author', 'Multitab reviewer'));
    const pages: Page[] = [];
    const streams: string[] = [];
    const socketPages = new Set<Page>();
    const socketUrls = new Map<Page, string[]>();
    let held = false;
    let release!: () => void;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    try {
      context.on('request', (request) => {
        if (request.headers().accept === 'text/event-stream') streams.push(request.url());
      });
      for (let i = 0; i < 8; i++) {
        const page = await context.newPage();
        page.on('websocket', (socket) => {
          socketUrls.set(page, [...(socketUrls.get(page) ?? []), socket.url()]);
          socket.on('framereceived', () => socketPages.add(page));
        });
        pages.push(page);
        await page.goto(`${uiOrigin}${fixture.path}`);
        await expect(body(page)).toBeVisible();
        await expect(page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
      }
      await context.route(/\/document-commands(?:\?|$)/, async (route) => {
        held = true;
        await gate;
        await route.continue().catch(() => {});
      });
      await select(pages[0], first.block_id, 0);
      await pages[0].keyboard.insertText('Browser one ');
      await expect.poll(() => held).toBe(true);
      await expect(pages[0].getByLabel('Save status', { exact: true })).toHaveText('Saving…');
      await select(pages[1], second.block_id, second.text.length);
      await pages[1].keyboard.insertText(' Browser two');
      // A separate HTTP agent attaches to its original read during pending browser edits.
      await transaction(request, fixture.url, [{ op: 'comment.add', block_id: first.block_id,
        start: 7, end: 13, body: 'Original target across eight tabs' }], first.document_clock);
      release();
      for (const page of pages) {
        await expect(body(page)).toContainText('Browser one Before TARGET after.');
        await expect(body(page)).toContainText('Second paragraph. Browser two');
        await expect(body(page).locator('[data-comment-id]')).toHaveText('TARGET');
        await expect(page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
      }
      await expect.poll(() => socketPages.size).toBe(8);
      for (const page of pages) {
        const urls = socketUrls.get(page)!;
        // A temporary document also has the library sidebar's subscription.
        // Consumers of either scope must share that scope's connection.
        expect(new Set(urls).size === urls.length).toBe(true);
        expect(urls.filter((url) => new URL(url).pathname === (scope === 'library' ? '/v1/events' : new URL(fixture.url).pathname + '/events/stream'))).toHaveLength(1);
      }
      expect(streams).toEqual([]);
      expect((await review(request, fixture.url)).comments[0].target.state).toBe('attached');
      await pages[0].reload();
      await expect(body(pages[0])).toContainText('Browser one Before TARGET after.');
      await expect(body(pages[0]).locator('[data-comment-id]')).toHaveText('TARGET');
      await expect(pages[0].getByLabel('Save status', { exact: true })).toHaveText('Saved');
      await pages[7].close();
      await select(pages[0], first.block_id, 0);
      await pages[0].keyboard.insertText('Reloaded ');
      await expect.poll(async () => (await blocks(request, fixture.url))[0].text).toBe('Reloaded Browser one Before TARGET after.');
      await expect(pages[0].getByLabel('Save status', { exact: true })).toHaveText('Saved');
    } finally { release(); await context.close(); }
  });
}

test('blocked notifications still allow saves and refresh agent changes without HTTP streams', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'TARGET');
  const [first] = await blocks(request, fixture.url);
  const context = await browser.newContext();
  await context.addInitScript(() => {
    Object.defineProperty(window, 'WebSocket', { value: class { constructor() { throw new Error('Notification transport unavailable'); } } });
    Object.defineProperty(window, 'EventSource', { value: class { constructor() { throw new Error('HTTP streams must never be opened'); } } });
  });
  try {
    const page = await context.newPage();
    await page.goto(`${uiOrigin}${fixture.path}`);
    await expect(body(page)).toHaveText('TARGET');
    await select(page, first.block_id, 0);
    await page.keyboard.insertText('Browser ');
    await expect(page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    const [current] = await blocks(request, fixture.url);
    await transaction(request, fixture.url, [{ op: 'replace_block_content', block_id: first.block_id, text: current.text + ' Agent' }], current.document_clock);
    await expect(body(page)).toHaveText('Browser TARGET Agent');
    await expect(page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
  } finally { await context.close(); }
});
