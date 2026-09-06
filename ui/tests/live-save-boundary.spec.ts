import { expect, test, type Page } from 'playwright/test';
import { apiOrigin, blocks, body, createDocument, openDocument, select, transaction, uiOrigin } from './helpers/native-document';

test.describe.configure({ timeout: 60000 });
async function pending(page: Page) {
  return page.evaluate(async () => {
    const database = await new Promise<IDBDatabase>((resolve, reject) => {
      const request = indexedDB.open('quarry-document-outbox');
      request.onsuccess = () => resolve(request.result); request.onerror = () => reject(request.error);
    });
    try { return await new Promise<number>((resolve, reject) => {
      const read = database.transaction('requests').objectStore('requests').getAll();
      read.onsuccess = () => resolve(read.result.filter((row: { state: string }) => row.state === 'pending').length);
      read.onerror = () => reject(read.error);
    }); } finally { database.close(); }
  });
}

for (const scope of ['library', 'temporary']) test(`${scope}: held and failed refreshes cannot block saves or misreport acknowledgements`, async ({ browser, request }) => {
  const fixture = scope === 'library' ? await createDocument(request, 'TARGET') : await (async () => {
    const response = await request.post(`${apiOrigin}/v1/tmp/documents`, { data: { content: 'TARGET' } });
    expect(response.ok()).toBe(true); const { document } = await response.json();
    return { url: `${apiOrigin}/v1/tmp/documents/${document.path}`, path: `/tmp/${document.path}` };
  })();
  const [block] = await blocks(request, fixture.url);
  const user = await openDocument(browser, fixture.path, 'Writer');
  let release!: () => void, held = false;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  try {
    await user.page.route(/\/document-state(?:\?|$)/, async (route) => {
      if (held) { await route.continue(); return; }
      held = true; await gate;
      await route.fulfill({ status: 503, contentType: 'application/json', body: JSON.stringify({ message: 'Refresh unavailable' }) }).catch(() => {});
    });
    await user.page.evaluate(() => window.dispatchEvent(new Event('online')));
    await expect.poll(() => held).toBe(true);
    for (const prefix of ['First ', 'Second ']) {
      await select(user.page, block.block_id, 0); await user.page.keyboard.insertText(prefix);
      await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved', { timeout: 5000 });
    }
    expect((await blocks(request, fixture.url))[0].text).toBe('Second First TARGET');
    expect(await pending(user.page)).toBe(0);
    release();
    await expect(user.page.getByLabel('Update status', { exact: true })).toHaveText('Reconnecting for updates…');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await expect(user.page.getByRole('button', { name: 'Download draft', exact: true })).toHaveCount(0);
    await expect(user.page.getByLabel('Update status', { exact: true })).toHaveCount(0, { timeout: 10000 });
    await user.page.reload(); await expect(body(user.page)).toHaveText('Second First TARGET');
    expect(user.errors).toEqual([]);
  } finally { release(); await user.context.close(); }
});

test('healthy sockets may lose every notification and periodic reads still recover the exact comment target', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'TARGET');
  const [block] = await blocks(request, fixture.url);
  const context = await browser.newContext();
  await context.addInitScript(() => {
    const NativeSocket = window.WebSocket;
    class SilentSocket extends NativeSocket {
      constructor(url: string | URL, protocols?: string | string[]) {
        super(url, protocols);
        this.addEventListener('open', () => sessionStorage.setItem('notifications-open', 'yes'));
        this.addEventListener('message', () => { sessionStorage.setItem('discarded-notifications', String(Number(sessionStorage.getItem('discarded-notifications') ?? 0) + 1)); });
        Object.defineProperty(this, 'onmessage', { get: () => null, set: () => {} });
      }
    }
    window.WebSocket = SilentSocket;
  });
  try {
    const page = await context.newPage();
    let stateReads = 0, lastReadAt = 0;
    const pendingReads = new Set<unknown>();
    page.on('request', (request) => {
      if (new URL(request.url()).pathname.endsWith('/document-state')) {
        stateReads++; lastReadAt = Date.now(); pendingReads.add(request);
      }
    });
    page.on('requestfinished', (request) => pendingReads.delete(request));
    await page.goto(`${uiOrigin}${fixture.path}`); await expect(body(page)).toHaveText('TARGET');
    await expect.poll(() => page.evaluate(() => sessionStorage.getItem('notifications-open'))).toBe('yes');
    await expect.poll(() => stateReads >= 2 && pendingReads.size === 0).toBe(true);
    await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
    const initialReadAt = lastReadAt;
    await transaction(request, fixture.url, [{ op: 'comment.add', block_id: block.block_id, start: 0, end: 6, body: 'Original characters' },
      { op: 'replace_block_content', block_id: block.block_id, text: 'Agent TARGET' }], block.document_clock);
    await expect.poll(() => page.evaluate(() => Number(sessionStorage.getItem('discarded-notifications') ?? 0))).toBeGreaterThan(0);
    await expect(body(page)).toHaveText('Agent TARGET', { timeout: 22000 });
    // Initial loading or a reconnect must not make this polling test pass.
    expect(lastReadAt - initialReadAt).toBeGreaterThanOrEqual(10000);
    await expect(body(page).locator('[data-comment-id]')).toHaveText('TARGET');
    await expect(page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
  } finally { await context.close(); }
});

test('a response that never arrives times out and replays the exact committed request before later edits', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'TARGET');
  const [block] = await blocks(request, fixture.url);
  const before = await (await request.get(`${fixture.url}/versions`)).json();
  const user = await openDocument(browser, fixture.path, 'Writer');
  const envelopes: string[] = [];
  let release!: () => void, committed = false;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  try {
    await user.page.route(/\/document-commands(?:\?|$)/, async (route) => {
      envelopes.push(route.request().postData()!);
      const response = await route.fetch();
      if (envelopes.length === 1) { committed = true; await gate; }
      await route.fulfill({ response }).catch(() => {});
    });
    await select(user.page, block.block_id, 0); await user.page.keyboard.insertText('First ');
    await expect.poll(() => committed).toBe(true);
    await select(user.page, block.block_id, 0); await user.page.keyboard.insertText('Second ');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved', { timeout: 20000 });
    expect(envelopes).toHaveLength(3); expect(envelopes[1]).toBe(envelopes[0]); expect(envelopes[2]).not.toBe(envelopes[0]);
    expect((await blocks(request, fixture.url))[0].text).toBe('Second First TARGET');
    expect(await pending(user.page)).toBe(0);
    expect(await (await request.get(`${fixture.url}/versions`)).json()).toHaveLength(before.length + 2);
    release();
    await user.page.reload(); await expect(body(user.page)).toHaveText('Second First TARGET');
    expect(user.errors).toEqual([]);
  } finally { release(); await user.context.close(); }
});

test('another tab takes over a closed delivering tab without duplicating its committed edit', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'TARGET');
  const [block] = await blocks(request, fixture.url);
  const before = await (await request.get(`${fixture.url}/versions`)).json();
  const writer = await openDocument(browser, fixture.path, 'Writer');
  const other = await writer.context.newPage();
  await other.goto(`${uiOrigin}${fixture.path}`); await expect(body(other)).toHaveText('TARGET');
  await expect(other.getByLabel('Save status', { exact: true })).toHaveText('Saved');
  const envelopes: string[] = [];
  let deliveringPage: Page | undefined;
  let release!: () => void, committed = false;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  try {
    await writer.context.route(/\/document-commands(?:\?|$)/, async (route) => {
      envelopes.push(route.request().postData()!);
      const response = await route.fetch();
      if (envelopes.length === 1) { deliveringPage = route.request().frame().page(); committed = true; await gate; }
      await route.fulfill({ response }).catch(() => {});
    });
    await select(writer.page, block.block_id, 0); await writer.page.keyboard.insertText('Kept ');
    await expect.poll(() => committed).toBe(true);
    // Either tab can win the shared outbox lock. Close the actual sender.
    const survivor = deliveringPage === writer.page ? other : writer.page;
    await deliveringPage!.close();
    await expect.poll(() => envelopes.length).toBeGreaterThanOrEqual(2);
    await expect(survivor.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await expect(body(survivor)).toHaveText('Kept TARGET');
    expect(new Set(envelopes).size).toBe(1); expect(await pending(survivor)).toBe(0);
    expect(await (await request.get(`${fixture.url}/versions`)).json()).toHaveLength(before.length + 1);
  } finally { release(); await writer.context.close(); }
});
