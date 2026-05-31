import { expect, test, type Page, type Route } from 'playwright/test';

// End-to-end proof of the review round-trip: a document with CriticMarkup review
// marks loads with the right editor marks, and the highlight / comment / suggest /
// accept controls (added in Tasks 6/7/9) persist back to Markdown through the RFM
// codec. This mirrors tests/workspace.spec.ts: a mocked /v1 API serves and stores
// documents, and the workspace loads them by clicking the tree item. The save flow
// is a conditional PUT, so we capture the last PUT body to assert against the
// persisted Markdown — the actual round-trip contract.

interface MockDocument {
  content: string;
  id: string;
  metadata?: Record<string, unknown>;
  path: string;
  version: string;
}

// The seed document from the review unit tests: a commented "here" range plus the
// matching `comments:` endmatter so the comment mark survives the codec.
const COMMENTED_DOC =
  'See {==here==}{>>fix this<<}{#c1}.\n\n---\ncomments:\n  c1:\n    at: "2026-01-01T00:00:00.000Z"\n    by: user\n';

test.describe('Review round-trip', () => {
  test.beforeEach(async ({ page }) => {
    await disableEventSource(page);
  });

  test('renders the commented range with the comment mark on load', async ({ page }) => {
    await installMockApi(page, [
      { content: COMMENTED_DOC, id: 'doc-review', metadata: { title: 'Review' }, path: 'review.md', version: 'v1' },
    ]);

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Review/ }).click();

    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('here');
    // Plate's CommentPlugin styles the commented leaf with `slate-comment`.
    const commented = editor.locator('.slate-comment');
    await expect(commented).toHaveText('here');
  });

  test('highlights a selected word and persists {==word==}', async ({ page }) => {
    const saves = await installMockApi(page, [
      { content: 'Highlight target here.\n', id: 'doc-hl', metadata: { title: 'HL' }, path: 'hl.md', version: 'v1' },
    ]);

    await page.goto('/');
    await page.getByRole('treeitem', { name: /HL/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('Highlight target');

    // Selecting a word raises the floating toolbar with the review controls.
    await page.getByText('target', { exact: false }).dblclick();
    await page.getByRole('button', { name: 'Highlight', exact: true }).click();

    // The highlight leaf renders as a <mark> in the editor (Plate's default
    // highlight leaf), and the saved Markdown gains the CriticMarkup highlight.
    await expect(editor.locator('mark')).toHaveText('target');

    await page.getByRole('button', { name: 'Save document' }).click();
    await expect(page.locator('[aria-label="Save status"]')).toContainText('Saved');
    expect(saves.lastBody('hl.md')).toContain('{==target==}');
  });

  test('suggests inserted text that persists as {++…++}{#id} with a suggestions endmatter', async ({ page }) => {
    const saves = await installMockApi(page, [
      { content: 'Base sentence.\n', id: 'doc-sg', metadata: { title: 'SG' }, path: 'sg.md', version: 'v1' },
    ]);

    await page.goto('/');
    await page.getByRole('treeitem', { name: /SG/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('Base sentence');

    // Enter suggesting mode via the toolbar toggle (it lives in the same floating
    // toolbar, so raise the toolbar with a selection first), then type. While
    // suggesting, typed text becomes a suggestion (insertion) mark.
    await page.getByText('Base sentence', { exact: false }).dblclick();
    const suggest = page.getByTestId('suggest-toggle');
    await suggest.click();
    await expect(suggest).toHaveAttribute('aria-pressed', 'true');

    await editor.click();
    await page.keyboard.press('End');
    await page.keyboard.type(' added');
    await expect(editor).toContainText('added');

    await page.getByRole('button', { name: 'Save document' }).click();
    await expect(page.locator('[aria-label="Save status"]')).toContainText('Saved');

    // The insertion serializes to `{++…++}{#id}` and records a `suggestions:`
    // endmatter entry — the codec's round-trip for a live suggestion.
    const saved = saves.lastBody('sg.md');
    expect(saved).toMatch(/\{\+\+[^}]*added[^}]*\+\+\}\{#[^}]+\}/);
    expect(saved).toContain('suggestions:');
  });

  test('accepts a suggestion: applies the text and drops the markup', async ({ page }) => {
    // Load a document that already carries an insertion suggestion plus its
    // endmatter, so the Accept control is reachable from a selection inside it.
    const suggestedDoc =
      'Keep {++this++}{#s1} text.\n\n---\nsuggestions:\n  s1:\n    at: "2026-01-01T00:00:00.000Z"\n    by: user\n';
    const saves = await installMockApi(page, [
      { content: suggestedDoc, id: 'doc-acc', metadata: { title: 'ACC' }, path: 'acc.md', version: 'v1' },
    ]);

    await page.goto('/');
    await page.getByRole('treeitem', { name: /ACC/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('this');

    // Put the selection inside the suggested word so the Accept control renders.
    await page.getByText('this', { exact: false }).dblclick();
    const accept = page.getByTestId('accept-suggestion');
    await expect(accept).toBeVisible();
    await accept.click();

    // The Accept control disappears (no suggestion under the selection) and the
    // suggested text remains in the editor.
    await expect(page.getByTestId('accept-suggestion')).toHaveCount(0);
    await expect(editor).toContainText('Keep this text');

    await page.getByRole('button', { name: 'Save document' }).click();
    await expect(page.locator('[aria-label="Save status"]')).toContainText('Saved');

    // Accepting an insertion keeps the text but removes the CriticMarkup and the
    // suggestion endmatter from the persisted Markdown.
    const saved = saves.lastBody('acc.md');
    expect(saved).toContain('Keep this text.');
    expect(saved).not.toContain('{++');
    expect(saved).not.toContain('suggestions:');
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
