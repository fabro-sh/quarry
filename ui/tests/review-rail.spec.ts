import { expect, test, type Page, type Route } from 'playwright/test';

import {
  installMockCollabServer,
  type MockCollabServer,
  type MockRoomReviewMeta,
} from './helpers/mock-collab-server';

// End-to-end proof of the review rail: the rail lists comment threads and
// live suggestions for the document, and its controls (reply, resolve,
// accept) persist into the live session — marks in the shared doc plus
// entries in the shared review map, which the server checkpoint projects to
// rows. The mock collab server holds the room state in the test process, so
// the assertions read the exact payload a real checkpoint would receive.

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
// The server seeds the shared review map from canonical rows; the mock room
// mirrors that with the entry the endmatter/inline body would project to.
const COMMENTED_META: MockRoomReviewMeta = {
  comments: { c1: { by: 'user', at: '2026-01-01T00:00:00.000Z', body: 'fix this' } },
};

const SUGGESTED_DOC =
  'Use {~~rough~>specific~~}{#s1} wording.\n\n---\nsuggestions:\n  s1:\n    at: "2026-01-01T00:00:00.000Z"\n    by: user\n';
const SUGGESTED_META: MockRoomReviewMeta = {
  suggestions: { s1: { by: 'user', at: '2026-01-01T00:00:00.000Z' } },
};

test.describe('Review rail', () => {
  test.beforeEach(async ({ page }) => {
    await seedAuthor(page);
    await disableEventSource(page);
  });

  test('shows a comment card for the document thread', async ({ page }) => {
    await installMockApi(
      page,
      [{ content: COMMENTED_DOC, id: 'doc-rail', metadata: { title: 'Rail' }, path: 'rail.md', version: 'v1' }],
      { 'doc-rail': COMMENTED_META }
    );

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Rail/ }).click();
    await expect(page.getByLabel('Plate markdown editor')).toContainText('here');

    await expect(page.getByTestId('review-rail')).toBeVisible();
    const card = page.getByTestId('comment-card');
    await expect(card).toBeVisible();
    await expect(card).toContainText('fix this');
  });

  test('shows loaded suggestions before editor interaction', async ({ page }) => {
    await installMockApi(
      page,
      [{ content: SUGGESTED_DOC, id: 'doc-suggested', metadata: { title: 'Suggested' }, path: 'suggested.md', version: 'v1' }],
      { 'doc-suggested': SUGGESTED_META }
    );

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Suggested/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('rough');
    await expect(editor).toContainText('specific');
    await expect(editor.locator('[data-suggestion-id="s1"]')).toHaveCount(2);

    const card = page.getByTestId('suggestion-card');
    await expect(card).toBeVisible();
    await expect(card).toContainText('Replace:');
    await expect(card).toContainText('rough');
    await expect(card).toContainText('specific');
  });

  test('reply persists the new comment with a `re:` parent and body', async ({ page }) => {
    const collab = await installMockApi(
      page,
      [{ content: COMMENTED_DOC, id: 'doc-rail', metadata: { title: 'Rail' }, path: 'rail.md', version: 'v1' }],
      { 'doc-rail': COMMENTED_META }
    );

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Rail/ }).click();
    await expect(page.getByLabel('Plate markdown editor')).toContainText('here');

    // The reply composer only appears once the card is the active thread.
    await page.getByTestId('comment-card').click();
    await page.getByTestId('reply-input').fill('Looks good now');
    await page.getByTestId('reply-submit').click();

    // The reply lands in the shared review map as a sibling entry pointing
    // back at c1 — exactly what the server checkpoint projects to a reply row.
    await expect
      .poll(() => {
        const comments = collab.roomReviewMeta('doc-rail').comments ?? {};
        return Object.values(comments).some(
          (entry) => entry.re === 'c1' && entry.body === 'Looks good now'
        );
      })
      .toBe(true);
  });

  test('resolve persists `status: resolved` for the thread', async ({ page }) => {
    const collab = await installMockApi(
      page,
      [{ content: COMMENTED_DOC, id: 'doc-rail', metadata: { title: 'Rail' }, path: 'rail.md', version: 'v1' }],
      { 'doc-rail': COMMENTED_META }
    );

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Rail/ }).click();
    await expect(page.getByLabel('Plate markdown editor')).toContainText('here');

    // Resolve is a checkbox button in the card header, next to the actions menu.
    const resolve = page.getByTestId('resolve-comment');
    await expect(resolve).toBeVisible();
    await resolve.click();

    await expect
      .poll(() => collab.roomReviewMeta('doc-rail').comments?.c1?.status)
      .toBe('resolved');
  });

  test('delete removes the in-text mark and drops the comment from saved Markdown', async ({ page }) => {
    const collab = await installMockApi(
      page,
      [{ content: COMMENTED_DOC, id: 'doc-rail', metadata: { title: 'Rail' }, path: 'rail.md', version: 'v1' }],
      { 'doc-rail': COMMENTED_META }
    );

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

    // The session settles to Saved; the comment entry is gone from the
    // shared review map, so the checkpoint drops its row and it cannot
    // reappear on reload.
    await expect(page.locator('[aria-label="Save status"]')).toContainText('Saved');
    await expect.poll(() => collab.roomReviewMeta('doc-rail').comments?.c1).toBeUndefined();
  });

  test('commenting opens a draft composer that only persists on submit', async ({ page }) => {
    const collab = await installMockApi(page, [
      { content: 'Comment this word here.\n', id: 'doc-draft', metadata: { title: 'Draft' }, path: 'draft.md', version: 'v1' },
    ]);

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Draft/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('Comment this word');
    await expect(page.locator('[aria-label="Save status"]')).toContainText('Saved');

    // Select a word and raise the floating toolbar, then click Comment. This
    // sets a comment_draft mark (ignored by the codec) and opens the rail
    // composer — nothing is committed yet.
    await page.getByText('Comment this word here.', { exact: false }).dblclick();
    await page.getByTestId('comment-button').click();

    await expect(page.getByTestId('draft-composer')).toBeVisible();
    await expect(page.getByTestId('draft-input')).toBeFocused();
    await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())));
    await expect(page.getByTestId('draft-input')).toBeFocused();
    await expect(page.getByTestId('comment-card')).toHaveCount(0);

    // Nothing is committed while drafting: the draft mark is local-transient
    // (the codec ignores it), so no comment card exists yet.

    // Type a body and submit: the draft is promoted to a real comment card and,
    // on save, the CriticMarkup comment + endmatter reach the persisted Markdown.
    await page.getByTestId('draft-input').fill('Please clarify');
    await page.getByTestId('draft-submit').click();

    await expect(page.getByTestId('draft-composer')).toHaveCount(0);
    await expect(page.getByTestId('comment-card')).toBeVisible();
    await expect(page.getByTestId('comment-card')).toContainText('Please clarify');

    // Submission promotes the draft to a real comment mark + card, and the
    // new thread persists into the live session: an entry in the shared
    // review map (with the typed body) that the server checkpoint projects
    // to a comment row.
    await expect(editor.locator('[data-comment-id]')).toBeVisible();
    await expect
      .poll(() => {
        const comments = collab.roomReviewMeta('doc-draft').comments ?? {};
        return Object.values(comments).map((entry) => entry.body);
      })
      .toEqual(['Please clarify']);
  });

  test('cancelling a draft discards it and persists no comment', async ({ page }) => {
    const collab = await installMockApi(page, [
      { content: 'Cancel this draft please.\n', id: 'doc-cancel', metadata: { title: 'Cancel' }, path: 'cancel.md', version: 'v1' },
    ]);

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Cancel/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('Cancel this draft');
    await expect(page.locator('[aria-label="Save status"]')).toContainText('Saved');

    await page.getByText('Cancel this draft please.', { exact: false }).dblclick();
    await page.getByTestId('comment-button').click();
    await expect(page.getByTestId('draft-composer')).toBeVisible();

    await page.getByTestId('draft-cancel').click();
    await expect(page.getByTestId('draft-composer')).toHaveCount(0);
    await expect(page.getByTestId('comment-card')).toHaveCount(0);

    // Cancelling leaves the document unchanged: no comment mark, no card,
    // and nothing in the session's shared review map.
    await expect(editor.locator('[data-comment-id]')).toHaveCount(0);
    expect(collab.roomReviewMeta('doc-cancel').comments ?? {}).toEqual({});
  });

  test('accept from the rail applies the suggestion and drops the markup', async ({ page }) => {
    const collab = await installMockApi(page, [
      { content: 'Base sentence.\n', id: 'doc-acc', metadata: { title: 'AccRail' }, path: 'acc.md', version: 'v1' },
    ]);

    await page.goto('/');
    await page.getByRole('treeitem', { name: /AccRail/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('Base sentence');

    // Switch to Suggesting mode via the document mode selector, then type so the
    // inserted text becomes a suggestion.
    const mode = page.getByRole('button', { name: 'Document mode' });
    await mode.click();
    await page.getByRole('menuitem', { name: 'Suggesting' }).click();
    await expect(mode).toContainText('Suggesting');

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

    // The session settles to Saved with the accepted text in the room doc
    // and no suggestion entry left in the shared review map.
    await expect(page.locator('[aria-label="Save status"]')).toContainText('Saved');
    await expect.poll(() => collab.roomText('doc-acc')).toContain('added');
    expect(collab.roomReviewMeta('doc-acc').suggestions ?? {}).toEqual({});
  });

  test('hovering a comment card highlights the matching in-text mark', async ({ page }) => {
    await installMockApi(
      page,
      [{ content: COMMENTED_DOC, id: 'doc-rail', metadata: { title: 'Rail' }, path: 'rail.md', version: 'v1' }],
      { 'doc-rail': COMMENTED_META }
    );

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

async function seedAuthor(page: Page, name = 'Tester') {
  await page.addInitScript((author) => {
    window.localStorage.setItem('quarry:author', author);
  }, name);
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
      await route.fulfill({
        body: document.content,
        headers: {
          ETag: `"${document.version}"`,
          'content-type': 'text/markdown',
          'x-quarry-document-id': document.id,
        },
      });
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
