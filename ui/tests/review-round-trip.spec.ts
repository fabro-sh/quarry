import { expect, test, type Page, type Route } from 'playwright/test';

import {
  installMockCollabServer,
  type MockCollabServer,
  type MockRoomReviewMeta,
} from './helpers/mock-collab-server';

// End-to-end proof of the review round-trip: a document with CriticMarkup
// review marks loads with the right editor marks, and the suggest / accept
// controls persist into the live session — suggestion marks in the shared
// doc plus entries in the shared review map, which the server checkpoint
// projects to review rows. The mock collab server holds the room state in
// the test process, so the assertions read the exact payload a real
// checkpoint would receive.

interface MockDocument {
  // Selection-driven UI (dblclick to reach the floating Accept control) is
  // currently broken inside live-session editors (pre-existing slate-yjs
  // selection-sync bug, present since Phase 3 — see the Phase 5 report).
  // The floating-accept test opts out of collab; session persistence of
  // accepts is covered by the rail-accept test in review-rail.spec.ts.
  collab?: boolean;
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
const COMMENTED_META: MockRoomReviewMeta = {
  comments: { c1: { by: 'user', at: '2026-01-01T00:00:00.000Z', body: 'fix this' } },
};

test.describe('Review round-trip', () => {
  test.beforeEach(async ({ page }) => {
    await disableEventSource(page);
  });

  test('renders the commented range with the comment mark on load', async ({ page }) => {
    await installMockApi(
      page,
      [{ content: COMMENTED_DOC, id: 'doc-review', metadata: { title: 'Review' }, path: 'review.md', version: 'v1' }],
      { 'doc-review': COMMENTED_META }
    );

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Review/ }).click();

    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('here');
    // Plate's CommentPlugin styles the commented leaf with `slate-comment`.
    const commented = editor.locator('.slate-comment');
    await expect(commented).toHaveText('here');
  });

  test('suggests inserted text that persists as {++…++}{#id} with a suggestions endmatter', async ({ page }) => {
    const collab = await installMockApi(page, [
      { content: 'Base sentence.\n', id: 'doc-sg', metadata: { title: 'SG' }, path: 'sg.md', version: 'v1' },
    ]);

    await page.goto('/');
    await page.getByRole('treeitem', { name: /SG/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('Base sentence');

    // Switch to Suggesting mode via the document mode selector in the header.
    // While suggesting, typed text becomes a suggestion (insertion) mark.
    const mode = page.getByRole('button', { name: 'Document mode' });
    await mode.click();
    await page.getByRole('menuitem', { name: 'Suggesting' }).click();
    await expect(mode).toContainText('Suggesting');

    await editor.click();
    await page.keyboard.press('End');
    await page.keyboard.type(' added');
    await expect(editor).toContainText('added');

    // The session settles to Saved with the typed insertion in the room doc
    // and a suggestion entry in the shared review map — the payload the
    // server checkpoint projects to a suggestion row.
    await expect(page.locator('[aria-label="Save status"]')).toContainText('Saved');
    await expect.poll(() => collab.roomText('doc-sg')).toContain('added');
    await expect
      .poll(() => Object.keys(collab.roomReviewMeta('doc-sg').suggestions ?? {}).length)
      .toBeGreaterThan(0);
  });

  test('accepts a suggestion: applies the text and drops the markup', async ({ page }) => {
    // Load a document that already carries an insertion suggestion plus its
    // endmatter, so the Accept control is reachable from a selection inside it.
    const suggestedDoc =
      'Keep {++this++}{#s1} text.\n\n---\nsuggestions:\n  s1:\n    at: "2026-01-01T00:00:00.000Z"\n    by: user\n';
    await installMockApi(page, [
      { collab: false, content: suggestedDoc, id: 'doc-acc', metadata: { title: 'ACC' }, path: 'acc.md', version: 'v1' },
    ]);

    await page.goto('/');
    await page.getByRole('treeitem', { name: /ACC/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('this');

    // Put the selection inside the suggested word so the Accept control renders.
    // Scope to the editor: the rail's suggestion card also renders this word.
    await editor.getByText('this', { exact: false }).dblclick();
    const accept = page.getByTestId('accept-suggestion');
    await expect(accept).toBeVisible();
    await accept.click();

    // The Accept control disappears (no suggestion under the selection) and the
    // suggested text remains in the editor.
    await expect(page.getByTestId('accept-suggestion')).toHaveCount(0);
    await expect(editor).toContainText('Keep this text');

    // The accepted text stays as plain prose with the suggestion marks gone.
    // (Session persistence of accepts — text in the room doc, entry removed
    // from the shared review map — is covered by review-rail's rail-accept.)
    await expect(editor.locator('[data-suggestion-id="s1"]')).toHaveCount(0);
  });
});

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

// A trimmed mock of the /v1 API plus an in-test collab session server: the
// documents are served collab-enabled (x-quarry-document-id), and review
// persistence is asserted against the session room state — the new
// durability boundary (the legacy autosave PUT capture died with Phase 5).
async function installMockApi(
  page: Page,
  documents: MockDocument[],
  reviewMeta: Record<string, MockRoomReviewMeta> = {}
): Promise<MockCollabServer> {
  const collab = await installMockCollabServer(page, { reviewMeta });
  const libraries = [{ id: 'lib-notes', slug: 'notes' }];
  const store = new Map(documents.map((document) => [document.path, { ...document }]));

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
    if (path.endsWith('/review')) {
      await route.fulfill({
        json: { documentId: '', baseToken: '', comments: [], suggestions: [], conflicts: [] },
      });
      return;
    }

    const docPath = documentPath(path);
    const document = store.get(docPath);

    if (docPath && request.method() === 'GET') {
      if (!document) {
        await notFound(route);
        return;
      }
      const headers: Record<string, string> = {
        ETag: `"${document.version}"`,
        'content-type': 'text/markdown',
      };
      if (document.collab !== false) headers['x-quarry-document-id'] = document.id;
      await route.fulfill({ body: document.content, headers });
      return;
    }

    await notFound(route);
  });

  return collab;
}

function documentPath(path: string): string {
  const prefix = '/v1/libraries/notes/documents/';
  if (!path.startsWith(prefix)) return '';
  const remaining = path.slice(prefix.length);
  if (/\/(?:backlinks|outgoing-links|versions|review|move)$/.test(remaining)) return '';
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
