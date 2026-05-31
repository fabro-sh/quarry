import { expect, test, type Page, type Route } from 'playwright/test';

// End-to-end proof of the review rail (Plan 3): the rail lists comment threads
// and live suggestions for the document, and its controls (reply, resolve,
// accept) persist back to Markdown through the RFM codec, while hovering a card
// syncs to the in-text mark. This mirrors tests/review-round-trip.spec.ts: a
// mocked /v1 API serves and stores documents, the workspace loads them by
// clicking the tree item, and the save flow is a conditional PUT whose body we
// capture to assert against the persisted Markdown.

interface MockDocument {
  content: string;
  id: string;
  metadata?: Record<string, unknown>;
  path: string;
  version: string;
}

// A commented "here" range plus the matching `comments:` endmatter so the
// comment mark survives the codec and the rail hydrates `c1` from the store.
const COMMENTED_DOC =
  'See {==here==}{>>fix this<<}{#c1}.\n\n---\ncomments:\n  c1:\n    at: "2026-01-01T00:00:00.000Z"\n    by: user\n';

test.describe('Review rail', () => {
  test.beforeEach(async ({ page }) => {
    await disableEventSource(page);
  });

  test('shows a comment card for the document thread', async ({ page }) => {
    await installMockApi(page, [
      { content: COMMENTED_DOC, id: 'doc-rail', metadata: { title: 'Rail' }, path: 'rail.md', version: 'v1' },
    ]);

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Rail/ }).click();
    await expect(page.getByLabel('Plate markdown editor')).toContainText('here');

    await expect(page.getByTestId('review-rail')).toBeVisible();
    const card = page.getByTestId('comment-card');
    await expect(card).toBeVisible();
    await expect(card).toContainText('fix this');
  });

  test('reply persists the new comment with a `re:` parent and body', async ({ page }) => {
    const saves = await installMockApi(page, [
      { content: COMMENTED_DOC, id: 'doc-rail', metadata: { title: 'Rail' }, path: 'rail.md', version: 'v1' },
    ]);

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Rail/ }).click();
    await expect(page.getByLabel('Plate markdown editor')).toContainText('here');

    await page.getByTestId('reply-input').fill('Looks good now');
    await page.getByTestId('reply-submit').click();

    await page.getByRole('button', { name: 'Save document' }).click();
    await expect(page.locator('[aria-label="Save status"]')).toContainText('Saved');

    // The reply serializes as a sibling comment entry pointing back at c1 via
    // `re:`, carrying the reply body — the store's reply round-trip.
    const saved = saves.lastBody('rail.md');
    expect(saved).toContain('re: c1');
    expect(saved).toContain('Looks good now');
  });

  test('resolve persists `status: resolved` for the thread', async ({ page }) => {
    const saves = await installMockApi(page, [
      { content: COMMENTED_DOC, id: 'doc-rail', metadata: { title: 'Rail' }, path: 'rail.md', version: 'v1' },
    ]);

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Rail/ }).click();
    await expect(page.getByLabel('Plate markdown editor')).toContainText('here');

    // Resolve lives behind the card's actions dropdown (Radix). Open it, then
    // select the Resolve item.
    await page.getByRole('button', { name: 'Comment actions' }).click();
    const resolve = page.getByTestId('resolve-comment');
    await expect(resolve).toBeVisible();
    await resolve.click();

    await page.getByRole('button', { name: 'Save document' }).click();
    await expect(page.locator('[aria-label="Save status"]')).toContainText('Saved');

    const saved = saves.lastBody('rail.md');
    expect(saved).toContain('status: resolved');
  });

  test('delete removes the in-text mark and drops the comment from saved Markdown', async ({ page }) => {
    const saves = await installMockApi(page, [
      { content: COMMENTED_DOC, id: 'doc-rail', metadata: { title: 'Rail' }, path: 'rail.md', version: 'v1' },
    ]);

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Rail/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('here');
    await expect(editor.locator('[data-comment-id="c1"]')).toBeVisible();

    // Delete lives behind the card's actions dropdown (Radix). Open it, then
    // select the Delete item.
    await page.getByRole('button', { name: 'Comment actions' }).click();
    await page.getByRole('menuitem', { name: 'Delete' }).click();

    // The in-text comment mark is gone (no more warn decoration) and so is the
    // rail card.
    await expect(editor.locator('[data-comment-id="c1"]')).toHaveCount(0);
    await expect(page.getByTestId('comment-card')).toHaveCount(0);

    await page.getByRole('button', { name: 'Save document' }).click();
    await expect(page.locator('[aria-label="Save status"]')).toContainText('Saved');

    // Removing the mark drops the CriticMarkup comment markup from the persisted
    // Markdown, so it won't reappear on reload.
    const saved = saves.lastBody('rail.md');
    expect(saved).not.toContain('{==');
    expect(saved).not.toContain('{#c1}');
  });

  test('accept from the rail applies the suggestion and drops the markup', async ({ page }) => {
    const saves = await installMockApi(page, [
      { content: 'Base sentence.\n', id: 'doc-acc', metadata: { title: 'AccRail' }, path: 'acc.md', version: 'v1' },
    ]);

    await page.goto('/');
    await page.getByRole('treeitem', { name: /AccRail/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('Base sentence');

    // Enter suggesting mode via the toolbar toggle (raise the toolbar with a
    // selection first), then type so the inserted text becomes a suggestion.
    await page.getByText('Base sentence', { exact: false }).dblclick();
    const suggest = page.getByTestId('suggest-toggle');
    await suggest.click();
    await expect(suggest).toHaveAttribute('aria-pressed', 'true');

    await editor.click();
    await page.keyboard.press('End');
    await page.keyboard.type(' added');
    await expect(editor).toContainText('added');

    // The new suggestion appears in the rail; accept it from there.
    const card = page.getByTestId('suggestion-card');
    await expect(card).toBeVisible();
    await page.getByTestId('rail-accept').click();

    // The suggestion card is gone and the inserted text remains as plain prose.
    // Accepting via `withoutSuggestions` strips the mark; suggesting mode staying
    // on only affects future edits, so no plain text is re-decorated.
    await expect(page.getByTestId('suggestion-card')).toHaveCount(0);
    await expect(editor).toContainText('added');

    await page.getByRole('button', { name: 'Save document' }).click();
    await expect(page.locator('[aria-label="Save status"]')).toContainText('Saved');

    // Accepting an insertion keeps the text but removes the CriticMarkup
    // insertion marker from the persisted Markdown.
    const saved = saves.lastBody('acc.md');
    expect(saved).toContain('added');
    expect(saved).not.toContain('{++');
  });

  test('hovering a comment card highlights the matching in-text mark', async ({ page }) => {
    await installMockApi(page, [
      { content: COMMENTED_DOC, id: 'doc-rail', metadata: { title: 'Rail' }, path: 'rail.md', version: 'v1' },
    ]);

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Rail/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('here');

    const mark = editor.locator('[data-comment-id="c1"]');
    await expect(mark).toHaveAttribute('data-hover', 'false');

    // Hovering the card sets the shared store hoverId, which the in-text leaf
    // reads to flip its data-hover.
    await page.getByTestId('comment-card').hover();
    await expect(mark).toHaveAttribute('data-hover', 'true');
  });
});

interface SaveRecorder {
  /** The body of the most recent PUT to a document path, or '' if none. */
  lastBody(path: string): string;
}

async function disableEventSource(page: Page) {
  await page.addInitScript(() => {
    class DisabledEventSource {
      onerror: ((event: Event) => void) | null = null;
      onopen: ((event: Event) => void) | null = null;

      constructor() {
        window.setTimeout(() => this.onerror?.(new Event('error')), 0);
      }

      addEventListener() {}
      close() {}
      removeEventListener() {}
    }

    Object.defineProperty(window, 'EventSource', {
      configurable: true,
      value: DisabledEventSource,
    });
  });
}

// A trimmed mock of the /v1 API: it serves the seeded documents and stores PUT
// bodies so each document's saved Markdown can be asserted directly.
async function installMockApi(page: Page, documents: MockDocument[]): Promise<SaveRecorder> {
  const libraries = [{ id: 'lib-notes', slug: 'notes' }];
  const store = new Map(documents.map((document) => [document.path, { ...document }]));
  const lastSaveBody = new Map<string, string>();

  await page.route('**/v1/**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = decodeURIComponent(url.pathname);

    if (path === '/v1/libraries') {
      await route.fulfill({ json: libraries.map((library) => ({ ...library, created_at: 'now', settings: {} })) });
      return;
    }

    if (path === '/v1/libraries/notes/documents' && request.method() === 'GET') {
      await route.fulfill({ json: Array.from(store.values(), documentStub) });
      return;
    }

    if (path.endsWith('/conflicts')) {
      await route.fulfill({ json: [] });
      return;
    }
    if (path.includes('/git/peers')) {
      await route.fulfill({ json: [] });
      return;
    }
    if (path.includes('/search/suggest')) {
      await route.fulfill({ json: [] });
      return;
    }
    if (path.includes('/search')) {
      await route.fulfill({ json: { results: [], cursor: null } });
      return;
    }
    if (path.endsWith('/outgoing-links') || path.endsWith('/backlinks')) {
      await route.fulfill({ json: { path: documentPath(path), links: [] } });
      return;
    }
    if (path.endsWith('/versions')) {
      await route.fulfill({ json: [] });
      return;
    }

    const docPath = documentPath(path);
    const document = store.get(docPath);

    if (docPath && request.method() === 'GET') {
      if (!document) {
        await notFound(route);
        return;
      }
      await route.fulfill({
        body: document.content,
        headers: { ETag: `"${document.version}"`, 'content-type': 'text/markdown' },
      });
      return;
    }

    if (docPath && request.method() === 'PUT') {
      const body = request.postData() ?? '';
      lastSaveBody.set(docPath, body);
      const next: MockDocument = {
        content: body,
        id: document?.id ?? `doc-${docPath}`,
        metadata: document?.metadata ?? {},
        path: docPath,
        version: 'v-saved',
      };
      store.set(docPath, next);
      await route.fulfill({ headers: { ETag: `"${next.version}"` }, json: writeOutcome(next) });
      return;
    }

    await notFound(route);
  });

  return { lastBody: (path: string) => lastSaveBody.get(path) ?? '' };
}

function documentPath(path: string): string {
  const prefix = '/v1/libraries/notes/documents/';
  if (!path.startsWith(prefix)) return '';
  const remaining = path.slice(prefix.length);
  if (/\/(?:backlinks|outgoing-links|versions|move)$/.test(remaining)) return '';
  return remaining;
}

function documentStub(document: MockDocument) {
  return {
    id: document.id,
    path: document.path,
    head_version_id: document.version,
    content_type: 'text/markdown',
    byte_size: document.content.length,
    content_hash: null,
    metadata: document.metadata ?? {},
    updated_at: 'now',
  };
}

function writeOutcome(document: MockDocument) {
  return {
    document: documentStub(document),
    transaction: {
      actor: null,
      committed_at: 'now',
      created_at: 'now',
      id: `tx-${document.version}`,
      library_id: 'lib-notes',
      message: null,
      provenance: {},
      source: 'rest',
      state: 'committed',
    },
    version: {
      byte_size: document.content.length,
      content_hash: null,
      content_type: 'text/markdown',
      created_at: 'now',
      document_id: document.id,
      id: document.version,
      inline_content: null,
      metadata: document.metadata ?? {},
      transaction_actor: null,
      transaction_message: null,
      transaction_provenance: {},
      transaction_source: 'rest',
      tx_id: `tx-${document.version}`,
    },
  };
}

async function notFound(route: Route) {
  await route.fulfill({ body: 'not found', status: 404 });
}
