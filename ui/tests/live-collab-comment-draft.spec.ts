// Regression for the reported two-human bug: one human clicking Comment
// popped an EMPTY draft composer in the other human's browser. The draft used
// to be a `comment_draft` mark on the document text, and document content
// syncs live over Yjs — so the not-yet-posted draft reached every peer. The
// draft is client-local editor state now; the other human must see nothing
// until the comment is actually posted.
//
// Harness helpers (uniqueLibrary, seedDocument, openHumanDocument,
// setCaretAfterText, documentApiUrl) are duplicated from
// live-collab-agent-smoke.spec.ts, which does not export them; the copies are
// trimmed to what this spec needs.

import { expect, test, type APIRequestContext, type Browser, type Page } from 'playwright/test';

const API_ORIGIN = `http://127.0.0.1:${process.env.QUARRY_LIVE_API_PORT ?? '7832'}`;
const DOCUMENT_PATH = 'draft-privacy.md';

test.describe.configure({ timeout: 120_000 });

test('a comment draft stays invisible to the other human until posted', async ({
  browser,
  request,
}, testInfo) => {
  const library = uniqueLibrary('draft-privacy', testInfo.workerIndex);
  await seedDocument(request, library, [
    '# Draft privacy',
    '',
    'Avery comments on this sentence here.',
    '',
  ]);

  const userA = await openHumanDocument(browser, library, 'Avery');
  const userB = await openHumanDocument(browser, library, 'Blair');
  try {
    await expect(editorOf(userA.page)).toContainText('this sentence here.');
    await expect(editorOf(userB.page)).toContainText('this sentence here.');

    // Avery selects a word and opens the composer — but does not post.
    await userA.page.getByText('Avery comments on this sentence here.', { exact: false }).dblclick();
    await userA.page.getByTestId('comment-button').click();
    await expect(userA.page.getByTestId('draft-composer')).toBeVisible();
    // The drafting highlight renders for Avery (decoration, not a document mark).
    await expect(editorOf(userA.page).locator('[data-comment-draft="true"]')).toBeVisible();

    // Round-trip a real edit from Blair to prove sync is flowing both ways —
    // the negative assertions below are meaningless on a dead pipe.
    await setCaretAfterText(userB.page, 'here.');
    await userB.page.keyboard.type(' ping');
    await expect(editorOf(userA.page)).toContainText('here. ping');

    // Avery's draft survives the remote edit; Blair sees no trace of it.
    await expect(userA.page.getByTestId('draft-composer')).toBeVisible();
    await expect(userB.page.getByTestId('draft-composer')).toHaveCount(0);
    await expect(editorOf(userB.page).locator('[data-comment-draft="true"]')).toHaveCount(0);

    // Only posting makes the comment appear for Blair.
    await userA.page.getByTestId('draft-input').fill('Needs a citation');
    await userA.page.getByTestId('draft-submit').click();
    await expect(userA.page.getByTestId('comment-card')).toContainText('Needs a citation');
    await expect(userB.page.getByTestId('comment-card')).toContainText('Needs a citation', {
      timeout: 20_000,
    });
    await expect(editorOf(userB.page).locator('[data-comment-id]')).toBeVisible();

    expect(userA.pageErrors).toEqual([]);
    expect(userB.pageErrors).toEqual([]);
  } finally {
    await userA.context.close();
    await userB.context.close();
  }
});

function uniqueLibrary(prefix: string, workerIndex: number) {
  return `${prefix}-${Date.now().toString(36)}-${workerIndex}`;
}

function editorOf(page: Page) {
  return page.getByLabel('Plate markdown editor');
}

// --- helpers duplicated from live-collab-agent-smoke.spec.ts (not exported there) ---

async function seedDocument(request: APIRequestContext, library: string, markdownLines: string[]) {
  const libraryResponse = await request.post(`${API_ORIGIN}/v1/libraries`, {
    data: { slug: library },
  });
  expect(libraryResponse.status()).toBe(201);

  const documentResponse = await request.put(documentApiUrl(library), {
    data: markdownLines.join('\n'),
    headers: {
      'content-type': 'text/markdown',
      'If-None-Match': '*',
    },
  });
  expect(documentResponse.ok()).toBeTruthy();
}

async function openHumanDocument(browser: Browser, library: string, author: string) {
  const context = await browser.newContext();
  await context.addInitScript((value) => {
    window.localStorage.setItem('quarry:author', value);
    window.localStorage.setItem('quarry:theme', 'light');
  }, author);
  const page = await context.newPage();
  const pageErrors: string[] = [];
  page.on('pageerror', (error) => pageErrors.push(String(error)));
  await page.goto(
    `/lib/${encodeURIComponent(library)}/documents/${encodeURIComponent(DOCUMENT_PATH)}`
  );
  // The editor is read-only until its session is live (connected + synced +
  // seed ack received); wait before typing.
  await expect(page.locator('[data-collab-save-state="saved"]')).toBeVisible({
    timeout: 20_000,
  });
  return { context, page, pageErrors };
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
