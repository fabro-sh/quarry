// Live Phase 5 specs: the browser half of the session-scoped model.
//
// - Disconnect makes the editor genuinely read-only with a
//   "Reconnecting (read-only)" indicator; reconnect mounts a FRESH Y.Doc that
//   reseeds from the canonical session — no duplication from a stale doc
//   merging back in, no local persistence of pending updates.
// - Blocks the row model stores as `raw_markdown` (wikilink-bearing
//   paragraphs) render their Markdown source read-only instead of empty
//   space, and survive checkpoints byte-for-byte.

import { expect, test, type APIRequestContext, type Browser, type Page } from 'playwright/test';

const API_ORIGIN = 'http://127.0.0.1:7832';
const DOCUMENT_PATH = 'reconnect.md';

test.describe.configure({ timeout: 120_000 });

test('disconnect goes read-only; reconnect reseeds a fresh doc from canonical state', async ({
  browser,
  request,
}, testInfo) => {
  const library = `live-reconnect-${Date.now().toString(36)}-${testInfo.workerIndex}`;
  await seedDocument(request, library, 'Reconnect target paragraph.\n');

  const user = await openDocument(browser, library);
  const editor = user.page.getByLabel('Plate markdown editor');
  await expect(editor).toContainText('Reconnect target paragraph.');

  await setCaretAfterText(user.page, 'Reconnect target paragraph.');
  await user.page.keyboard.type(' Typed before drop.');
  await expectSaved(user.page);
  await waitForPersistedMarkdown(request, library, 'Typed before drop.');

  // Sever the connection: the editor must become read-only and say so.
  // Offline emulation blocks the retry attempts; closing the established
  // socket forces the actual disconnect.
  await user.context.setOffline(true);
  await user.page.evaluate(() => {
    const sockets =
      (window as Window & { __quarrySockets?: WebSocket[] }).__quarrySockets ?? [];
    for (const socket of sockets) {
      // Only the collab socket: killing Vite's HMR socket makes the dev
      // client reload the page into an offline error page.
      if (socket.url.includes('/v1/collab/')) socket.close();
    }
  });
  await expect(user.page.locator('[data-collab-save-state="reconnecting"]')).toBeVisible({
    timeout: 20_000,
  });
  await expect(user.page.locator('[aria-label="Save status"]')).toContainText(
    'Reconnecting (read-only)'
  );
  await expect(
    user.page.locator('[aria-label="Plate markdown editor"][contenteditable="false"]')
  ).toBeVisible();

  // Reconnect: a fresh doc reseeds from the canonical session. Content is
  // exactly the checkpointed state — present once, never duplicated by a
  // stale local doc merging back in.
  await user.context.setOffline(false);
  await expect(user.page.locator('[data-collab-save-state="saved"]')).toBeVisible({
    timeout: 30_000,
  });
  await expect(editor).toContainText('Reconnect target paragraph. Typed before drop.');
  const occurrences = await user.page.evaluate(() => {
    const text =
      document.querySelector('[aria-label="Plate markdown editor"]')?.textContent ?? '';
    return text.split('Reconnect target paragraph.').length - 1;
  });
  expect(occurrences).toBe(1);

  // The reseeded session is editable and durable again.
  await setCaretAfterText(user.page, 'Typed before drop.');
  await user.page.keyboard.type(' Typed after reconnect.');
  await expectSaved(user.page);
  await waitForPersistedMarkdown(request, library, 'Typed after reconnect.');

  await user.context.close();
});

test('raw_markdown blocks render their source and survive checkpoints', async ({
  browser,
  request,
}, testInfo) => {
  const library = `live-raw-${Date.now().toString(36)}-${testInfo.workerIndex}`;
  await seedDocument(
    request,
    library,
    'Plain editable paragraph.\n\nLinked to [[guide|the guide]] inline.\n'
  );

  const user = await openDocument(browser, library);
  const editor = user.page.getByLabel('Plate markdown editor');
  await expect(editor).toContainText('Plain editable paragraph.');

  // The wikilink paragraph degraded to a raw_markdown row; the browser
  // renders its source instead of invisible empty space.
  const rawBlock = user.page.getByTestId('raw-markdown-block');
  await expect(rawBlock).toBeVisible();
  await expect(rawBlock).toContainText('[[guide|the guide]]');

  // Typing in a sibling block checkpoints without corrupting the raw block.
  await setCaretAfterText(user.page, 'Plain editable paragraph.');
  await user.page.keyboard.type(' Edited live.');
  await expectSaved(user.page);
  const markdown = await waitForPersistedMarkdown(request, library, 'Edited live.');
  expect(markdown).toContain('[[guide|the guide]]');

  await user.context.close();
});

async function seedDocument(request: APIRequestContext, library: string, markdown: string) {
  const libraryResponse = await request.post(`${API_ORIGIN}/v1/libraries`, {
    data: { slug: library },
  });
  expect(libraryResponse.status()).toBe(201);
  const documentResponse = await request.put(documentApiUrl(library), {
    data: markdown,
    headers: { 'content-type': 'text/markdown', 'If-None-Match': '*' },
  });
  expect(documentResponse.ok()).toBeTruthy();
}

async function openDocument(browser: Browser, library: string) {
  const context = await browser.newContext();
  await context.addInitScript(() => {
    window.localStorage.setItem('quarry:author', 'Avery');
    window.localStorage.setItem('quarry:theme', 'light');
    // Track sockets so the test can sever an ESTABLISHED collab connection
    // (offline emulation alone does not kill open websockets).
    const sockets: WebSocket[] = [];
    (window as Window & { __quarrySockets?: WebSocket[] }).__quarrySockets = sockets;
    const NativeWebSocket = window.WebSocket;
    window.WebSocket = class extends NativeWebSocket {
      constructor(url: string | URL, protocols?: string | string[]) {
        super(url, protocols);
        sockets.push(this);
      }
    } as typeof WebSocket;
  });
  const page = await context.newPage();
  await page.goto(
    `/lib/${encodeURIComponent(library)}/documents/${encodeURIComponent(DOCUMENT_PATH)}`
  );
  await expect(page.locator('[data-collab-save-state="saved"]')).toBeVisible({
    timeout: 20_000,
  });
  return { context, page };
}

async function expectSaved(page: Page) {
  await expect(page.locator('[aria-label="Save status"]')).toContainText('Saved', {
    timeout: 20_000,
  });
}

async function waitForPersistedMarkdown(
  request: APIRequestContext,
  library: string,
  text: string,
  timeout = 20_000
): Promise<string> {
  let latest = '';
  await expect
    .poll(async () => {
      const response = await request.get(documentApiUrl(library));
      if (!response.ok()) return '';
      latest = await response.text();
      return latest;
    }, { timeout })
    .toContain(text);
  return latest;
}

async function setCaretAfterText(page: Page, text: string) {
  await page.getByLabel('Plate markdown editor').focus();
  await page.evaluate((target) => {
    const editor = document.querySelector('[aria-label="Plate markdown editor"]');
    if (!editor) throw new Error('Plate markdown editor not found');
    const walker = document.createTreeWalker(editor, NodeFilter.SHOW_TEXT);
    let node = walker.nextNode();
    while (node) {
      const content = node.textContent ?? '';
      const index = content.indexOf(target);
      if (index !== -1) {
        const range = document.createRange();
        range.setStart(node, index + target.length);
        range.collapse(true);
        const selection = window.getSelection();
        if (!selection) throw new Error('Selection unavailable');
        selection.removeAllRanges();
        selection.addRange(range);
        (editor as HTMLElement).focus();
        return;
      }
      node = walker.nextNode();
    }
    throw new Error(`Text not found in editor: ${target}`);
  }, text);
}

function documentApiUrl(library: string) {
  return `${API_ORIGIN}/v1/libraries/${encodeURIComponent(library)}/documents/${encodeURIComponent(
    DOCUMENT_PATH
  )}`;
}
