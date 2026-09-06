import { expect, type APIRequestContext, type Browser, type Page } from 'playwright/test';

export const apiOrigin = `http://127.0.0.1:${process.env.QUARRY_LIVE_API_PORT ?? '7832'}`;
export const uiOrigin = process.env.QUARRY_PRODUCTION_UI === '1' ? apiOrigin : `http://127.0.0.1:${process.env.QUARRY_LIVE_UI_PORT ?? '5174'}`;
export interface Block { block_id: string; text: string; block_type: string; document_clock: string }
export async function createDocument(request: APIRequestContext, markdown: string) {
  const library = `native-${crypto.randomUUID()}`;
  expect((await request.post(`${apiOrigin}/v1/libraries`, { data: { slug: library } })).status()).toBe(201);
  const url = `${apiOrigin}/v1/libraries/${library}/documents/doc.md`;
  const put = await request.put(url, { data: markdown, headers: { 'Content-Type': 'text/markdown' } });
  expect(put.ok(), await put.text()).toBe(true);
  return { library, url, path: `/lib/${library}/documents/doc.md` };
}
export async function openDocument(browser: Browser, path: string, author: string) {
  const context = await browser.newContext();
  await context.addInitScript((name) => localStorage.setItem('quarry:author', name), author);
  const page = await context.newPage();
  const errors: string[] = [];
  let navigating = false;
  page.on('request', (request) => {
    if (request.isNavigationRequest() && request.frame() === page.mainFrame()) navigating = true;
  });
  page.on('framenavigated', (frame) => { if (frame === page.mainFrame()) navigating = false; });
  page.on('pageerror', (error) => {
    // WebKit labels a fetch denied during page teardown as an access-control
    // page error, even when its rejection is caught. Require an actual main
    // frame navigation and the exact same-origin native state endpoint.
    const match = error.stack?.match(/^Fetch API cannot load (http:\/\/[^\s]+) due to access control checks\./);
    if (navigating && browser.browserType().name() === 'webkit' && match) {
      const url = new URL(match[1]);
      if (url.origin === new URL(page.url()).origin && /^\/v1\/(?:libraries\/[^/]+\/documents-by-id|tmp\/documents)\/[^/]+\/document-state$/.test(url.pathname)) return;
    }
    errors.push(error.message);
  });
  page.on('console', (message) => {
    if (message.type() !== 'error') return;
    // Intentional offline and lost-response tests generate browser network logs.
    if (/^(Failed to load resource:|Load failed|NetworkError when attempting to fetch resource)/.test(message.text())) return;
    if (/^\[JavaScript Error: "(?:The connection to http:\/\/127\.0\.0\.1:\d+\/.* was interrupted while the page was loading\.|Firefox can’t establish a connection to the server at http:\/\/127\.0\.0\.1:\d+\/.*events\/stream\.)"/.test(message.text())) return;
    errors.push(message.text());
  });
  await page.goto(`${uiOrigin}${path}`);
  await expect(body(page)).toBeVisible();
  await expect(page.getByLabel('Save status', { exact: true })).toHaveText('Saved', { timeout: 20000 });
  return { context, page, errors };
}
export function body(page: Page) { return page.locator('.quarry-document-body [role="textbox"]'); }
export async function paste(page: Page, html: string, text: string) {
  await body(page).evaluate((element, { html, text }) => {
    const clipboardData = new DataTransfer();
    clipboardData.setData('text/html', html); clipboardData.setData('text/plain', text);
    const event = new ClipboardEvent('paste', { clipboardData, bubbles: true, cancelable: true });
    // Firefox drops constructor-supplied data on synthetic clipboard events.
    Object.defineProperty(event, 'clipboardData', { value: clipboardData });
    element.dispatchEvent(event);
    if (!event.defaultPrevented) {
      // Modern engines deliver the editable insertion in a separate event.
      const input = new InputEvent('beforeinput', { inputType: 'insertFromPaste', bubbles: true, cancelable: true });
      Object.defineProperty(input, 'dataTransfer', { value: clipboardData }); element.dispatchEvent(input);
    }
  }, { html, text });
}
export async function blocks(request: APIRequestContext, url: string): Promise<Block[]> {
  const response = await request.get(`${url}/blocks`); expect(response.ok(), await response.text()).toBe(true);
  const snapshot = await response.json();
  // Keep each fixture block paired with the exact read version used for its offsets.
  return snapshot.blocks.map((block: Block) => ({ ...block, document_clock: snapshot.document_clock }));
}
export async function transaction(request: APIRequestContext, url: string, ops: unknown[], base: string) {
  const response = await request.post(`${url}/transactions`, { data: {
    client_tx_id: crypto.randomUUID(), actor: { kind: 'agent', id: 'test-agent' }, ops,
    base_clock: base,
  } });
  expect(response.ok(), await response.text()).toBe(true); return response.json();
}
export async function select(page: Page, id: string, start: number, end = start) {
  await selectRange(page, { id, offset: start }, { id, offset: end });
}
export async function selectRange(page: Page, anchor: { id: string; offset: number }, focus: { id: string; offset: number }) {
  await page.evaluate(({ anchor, focus }) => {
    const point = ({ id, offset }: { id: string; offset: number }): [Node, number] => {
    const element = document.querySelector(`[data-block-id="${CSS.escape(id)}"],.slate-quarry_proposal[data-suggestion-id="${CSS.escape(id)}"]`)!;
    if (!element) throw new Error(`Missing block ${id}`);
    const editor = element.closest('[contenteditable]') as HTMLElement;
    // This helper represents a user selection, including a click at the same
    // caret position. Native selection synchronization alone is not a gesture.
    element.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
    editor.focus();
    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
    const nodes: Text[] = []; let node;
    while ((node = walker.nextNode())) {
      const parent = node.parentElement;
      if (!parent?.closest('[data-slate-string]') || !element.matches('.slate-quarry_proposal') && parent.closest('.slate-quarry_proposal')) continue;
      nodes.push(node as Text);
    }
      for (const node of nodes) { if (offset <= node.length) return [node, offset]; offset -= node.length; }
      if (!nodes.length && offset === 0) return [element, 0];
      throw new Error(`Selection is outside ${id}`);
    };
    const range = document.createRange(); range.setStart(...point(anchor)); range.setEnd(...point(focus));
    const selection = window.getSelection()!; selection.removeAllRanges(); selection.addRange(range);
    document.dispatchEvent(new Event('selectionchange'));
  }, { anchor, focus });
}
export async function review(request: APIRequestContext, url: string) {
  const response = await request.get(`${url}/review?includeResolved=1`); expect(response.ok()).toBe(true); return response.json();
}
