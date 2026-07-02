// Live smoke: two humans and an agent collaborate on one document.
//
// Phase 3 of the session-scoped collaboration rewrite: the agent writes
// through `POST .../transactions` (the semantic mutation gateway), which
// applies into the live session as another collaborator and checkpoints
// before acking. The legacy `/edit` and `/ops` facades are deleted, and
// review items persist as `block_review_items` rows (visible via
// `GET .../review`) instead of CriticMarkup in the document Markdown — the
// persistence assertions in this spec changed accordingly.

import { expect, test, type APIRequestContext, type Browser, type Page } from 'playwright/test';

interface BlockNodePayload {
  block_id: string;
  text: string;
}

interface BlockTreeResponse {
  document_id: string;
  document_clock: string;
  blocks: BlockNodePayload[];
}

interface BlockTransactionAck {
  status: string;
  document_clock: string;
  transaction_id: string;
  changed_block_ids: string[];
}

interface ReviewItem {
  id: string;
  status: string;
  body?: string;
  content?: string;
  quote: string;
  anchor?: { blockId: string; startOffset: number; endOffset: number };
}

interface ReviewResponse {
  comments: ReviewItem[];
  suggestions: ReviewItem[];
}

const API_ORIGIN = 'http://127.0.0.1:7832';
const DOCUMENT_PATH = 'shared.md';

const INITIAL_MARKDOWN = [
  '# Shared live smoke',
  '',
  'Human direct block target.',
  '',
  'Human suggesting block target.',
  '',
  'Agent direct block target.',
  '',
  'Agent comment target appears here.',
  '',
  'Review suggestion target here.',
  '',
  'Trailing stability block.',
  '',
].join('\n');

test.describe.configure({ timeout: 120_000 });

test('humans and an agent collaborate on one live document without conflicts', async ({
  browser,
  request,
}, testInfo) => {
  const library = `live-smoke-${Date.now().toString(36)}-${testInfo.workerIndex}`;
  await seedDocument(request, library);

  let userA = await openHumanDocument(browser, library, 'Avery');
  let userB = await openHumanDocument(browser, library, 'Blair');

  try {
    let editorA = userA.page.getByLabel('Plate markdown editor');
    let editorB = userB.page.getByLabel('Plate markdown editor');

    await expect(editorA).toContainText('Shared live smoke');
    await expect(editorB).toContainText('Shared live smoke');
    await expect(editorA).toContainText('Human direct block target.');
    await expect(editorB).toContainText('Human direct block target.');
    await expectNoConflictUi(userA.page);
    await expectNoConflictUi(userB.page);

    // Phase 5: there is no flusher — every browser is an equal session
    // participant whose typing persists through server checkpoints.
    const directWriter = userA;
    const directReader = userB;
    await setCaretAfterText(directWriter.page, 'Human direct block target.');
    await directWriter.page.keyboard.type(' from User A');

    await expect(directReader.page.getByLabel('Plate markdown editor')).toContainText(
      'Human direct block target. from User A'
    );
    await expectSaved(directWriter.page);
    await waitForPersistedMarkdown(request, library, 'from User A');
    await expectNoConflictUi(userA.page);
    await expectNoConflictUi(userB.page);

    const suggestingWriter = userB;
    const suggestingReader = userA;
    const mode = suggestingWriter.page.getByRole('button', { name: 'Document mode' });
    await mode.click();
    await suggestingWriter.page.getByRole('menuitem', { name: 'Suggesting' }).click();
    await expect(mode).toContainText('Suggesting');

    await setCaretAfterText(suggestingWriter.page, 'Human suggesting block target.');
    await suggestingWriter.page.keyboard.type('User B suggestion');

    await expect(suggestingReader.page.getByLabel('Plate markdown editor')).toContainText(
      'User B suggestion'
    );
    await expect(
      suggestingReader.page.getByTestId('suggestion-card').filter({ hasText: 'User B suggestion' })
    ).toBeVisible();
    await expectSaved(suggestingWriter.page);
    // Suggestions persist as review rows now, not as CriticMarkup in the
    // Markdown: the checkpoint projects the typed insertion suggestion into
    // `block_review_items`, visible through the review API.
    await waitForReview(request, library, (review) =>
      review.suggestions.some((suggestion) => suggestion.content === 'User B suggestion')
    );
    await expectNoConflictUi(userA.page);
    await expectNoConflictUi(userB.page);

    await userA.page.getByRole('button', { name: 'Add agent' }).click();
    const addAgentDialog = userA.page.getByRole('dialog', { name: 'Add agent' });
    await expect(addAgentDialog).toContainText(`/lib/${library}/documents/${DOCUMENT_PATH}`);
    for (const endpoint of ['/presence', '/blocks', '/transactions', '/review']) {
      await expect(addAgentDialog).toContainText(
        `/v1/libraries/${library}/documents/${DOCUMENT_PATH}${endpoint}`
      );
    }
    // The deleted legacy facades and the ordinal snapshot are not advertised.
    await expect(addAgentDialog).not.toContainText(`${DOCUMENT_PATH}/edit`);
    await expect(addAgentDialog).not.toContainText(`${DOCUMENT_PATH}/ops`);
    await expect(addAgentDialog).not.toContainText(`${DOCUMENT_PATH}/snapshot`);
    await userA.page.getByRole('button', { name: 'Close' }).click();

    const presence = await request.post(`${documentApiUrl(library)}/presence`, {
      data: { status: 'reading', by: 'Codex' },
      headers: { 'X-Agent-Id': 'ai:codex:smoke' },
    });
    expect(presence.ok()).toBeTruthy();
    await expect(userA.page.getByLabel(/Codex .* reading/)).toBeVisible();
    await expectNoConflictUi(userA.page);
    await expectNoConflictUi(userB.page);

    // The agent rewrites a block mid-session through the gateway: it lands
    // in the live doc as a collaborator edit and is durable at ack time.
    const directBlock = await blockContaining(request, library, 'Agent direct block target.');
    const edit = await postTransaction(request, library, 'tx-smoke-edit', [
      {
        op: 'replace_block_content',
        block_id: directBlock.block_id,
        text: 'Agent direct block target. Agent REST edit landed.',
      },
    ]);
    expect(edit.status).toBe('committed');
    expect(edit.changed_block_ids).toContain(directBlock.block_id);
    await waitForPersistedMarkdown(request, library, 'Agent REST edit landed');

    await expect(editorA).toContainText('Agent REST edit landed');
    await expect(editorB).toContainText('Agent REST edit landed');
    await expectNoConflictUi(userA.page);
    await expectNoConflictUi(userB.page);

    // Agent comment through the gateway: anchored review row + live marks.
    const commentBlock = await blockContaining(
      request,
      library,
      'Agent comment target appears here.'
    );
    const commentRange = rangeOf(commentBlock, 'comment target');
    const comment = await postTransaction(request, library, 'tx-smoke-comment', [
      {
        op: 'comment.add',
        block_id: commentBlock.block_id,
        start: commentRange.start,
        end: commentRange.end,
        body: 'Agent comment landed.',
        quote: 'comment target',
      },
    ]);
    expect(comment.status).toBe('committed');
    const commentId = (
      await waitForReview(request, library, (review) =>
        review.comments.some((item) => item.body === 'Agent comment landed.')
      )
    ).comments.find((item) => item.body === 'Agent comment landed.')!.id;

    await expect(
      userA.page.getByTestId('comment-card').filter({ hasText: 'Agent comment landed.' })
    ).toBeVisible();
    await expect(
      userB.page.getByTestId('comment-card').filter({ hasText: 'Agent comment landed.' })
    ).toBeVisible();
    await expect(editorA.locator(`[data-comment-id="${commentId}"]`)).toBeVisible();
    await expect(editorB.locator(`[data-comment-id="${commentId}"]`)).toBeVisible();
    await expectNoConflictUi(userA.page);
    await expectNoConflictUi(userB.page);

    const resolver = userB;
    const resolvedComment = resolver.page
      .getByTestId('comment-card')
      .filter({ hasText: 'Agent comment landed.' });
    await resolvedComment.hover();
    await resolvedComment.getByTestId('resolve-comment').click();

    await expect(
      userA.page.getByTestId('comment-card').filter({ hasText: 'Agent comment landed.' })
    ).toHaveCount(0);
    await expect(
      userB.page.getByTestId('comment-card').filter({ hasText: 'Agent comment landed.' })
    ).toHaveCount(0);
    // Resolution persists in the review rows (the session checkpoint), not
    // in document endmatter.
    await waitForReview(
      request,
      library,
      (review) =>
        review.comments.some((item) => item.id === commentId && item.status === 'resolved'),
      true,
      60_000
    );
    await userA.context.close();
    await userB.context.close();
    userA = await openHumanDocument(browser, library, 'Avery');
    userB = await openHumanDocument(browser, library, 'Blair');
    editorA = userA.page.getByLabel('Plate markdown editor');
    editorB = userB.page.getByLabel('Plate markdown editor');
    // A fresh session reseeds from rows: the resolved comment keeps its
    // anchor (so it can be reopened later) but earns no highlight and no
    // card — only open comments render `data-comment-id` (review-leaves.tsx,
    // "stop highlighting resolved comments in the editor").
    await expect(editorA).toContainText('Agent comment target appears here.');
    await expect(editorB).toContainText('Agent comment target appears here.');
    await expect(editorA.locator(`[data-comment-id="${commentId}"]`)).toHaveCount(0);
    await expect(editorB.locator(`[data-comment-id="${commentId}"]`)).toHaveCount(0);
    const resolvedReview = await waitForReview(
      request,
      library,
      (review) =>
        review.comments.some((item) => item.id === commentId && item.status === 'resolved'),
      true
    );
    const resolvedEntry = resolvedReview.comments.find((item) => item.id === commentId)!;
    expect(resolvedEntry.anchor).toBeTruthy();
    expect(resolvedEntry.anchor!.startOffset).toBeLessThan(resolvedEntry.anchor!.endOffset);
    await expect(
      userA.page.getByTestId('comment-card').filter({ hasText: 'Agent comment landed.' })
    ).toHaveCount(0);
    await expect(
      userB.page.getByTestId('comment-card').filter({ hasText: 'Agent comment landed.' })
    ).toHaveCount(0);
    await expectNoConflictUi(userA.page);
    await expectNoConflictUi(userB.page);

    // Agent suggestion through the gateway.
    const suggestionBlock = await blockContaining(
      request,
      library,
      'Review suggestion target here.'
    );
    const suggestionRange = rangeOf(suggestionBlock, 'suggestion target');
    const suggestion = await postTransaction(request, library, 'tx-smoke-suggestion', [
      {
        op: 'suggestion.add',
        block_id: suggestionBlock.block_id,
        start: suggestionRange.start,
        end: suggestionRange.end,
        replacement: 'agent suggestion replacement',
        quote: 'suggestion target',
      },
    ]);
    expect(suggestion.status).toBe('committed');
    const suggestionId = (
      await waitForReview(request, library, (review) =>
        review.suggestions.some((item) => item.content === 'agent suggestion replacement')
      )
    ).suggestions.find((item) => item.content === 'agent suggestion replacement')!.id;
    await userA.context.close();
    await userB.context.close();
    userA = await openHumanDocument(browser, library, 'Avery');
    userB = await openHumanDocument(browser, library, 'Blair');
    editorA = userA.page.getByLabel('Plate markdown editor');
    editorB = userB.page.getByLabel('Plate markdown editor');
    await expect(editorA).toContainText('agent suggestion replacement');
    await expect(editorB).toContainText('agent suggestion replacement');

    const agentSuggestionA = userA.page
      .getByTestId('suggestion-card')
      .filter({ hasText: 'agent suggestion replacement' });
    const agentSuggestionB = userB.page
      .getByTestId('suggestion-card')
      .filter({ hasText: 'agent suggestion replacement' });
    await expect(agentSuggestionA).toBeVisible();
    await expect(agentSuggestionB).toBeVisible();
    await expect(
      editorA.locator(`[data-suggestion-id="${suggestionId}"]`).first()
    ).toBeVisible();
    await expect(
      editorB.locator(`[data-suggestion-id="${suggestionId}"]`).first()
    ).toBeVisible();
    await expectNoConflictUi(userA.page);
    await expectNoConflictUi(userB.page);

    await agentSuggestionA.hover();
    await agentSuggestionA.getByTestId('rail-accept').click();

    await expect(agentSuggestionA).toHaveCount(0);
    await expect(agentSuggestionB).toHaveCount(0);
    await expect(editorA).toContainText('Review agent suggestion replacement here.');
    await expect(editorB).toContainText('Review agent suggestion replacement here.');
    await waitForPersistedMarkdown(
      request,
      library,
      'Review agent suggestion replacement here.',
      60_000
    );
    // Accepting through the editor removes the suggestion row entirely.
    await waitForReview(
      request,
      library,
      (review) => !review.suggestions.some((item) => item.id === suggestionId),
      true,
      60_000
    );
    await expectNoConflictUi(userA.page);
    await expectNoConflictUi(userB.page);
  } finally {
    await userA.context.close();
    await userB.context.close();
  }
});

async function seedDocument(request: APIRequestContext, library: string) {
  const libraryResponse = await request.post(`${API_ORIGIN}/v1/libraries`, {
    data: { slug: library },
  });
  expect(libraryResponse.status()).toBe(201);

  const documentResponse = await request.put(documentApiUrl(library), {
    data: INITIAL_MARKDOWN,
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
  await page.goto(
    `/lib/${encodeURIComponent(library)}/documents/${encodeURIComponent(DOCUMENT_PATH)}`
  );
  // The editor is read-only until its session is live (connected + synced +
  // seed ack received); wait before typing.
  await expect(page.locator('[data-collab-save-state="saved"]')).toBeVisible({
    timeout: 20_000,
  });
  return { context, page };
}

async function getBlocks(request: APIRequestContext, library: string): Promise<BlockTreeResponse> {
  const response = await request.get(`${documentApiUrl(library)}/blocks`);
  await expectOk(response, 'get blocks');
  return (await response.json()) as BlockTreeResponse;
}

async function blockContaining(
  request: APIRequestContext,
  library: string,
  text: string
): Promise<BlockNodePayload> {
  const tree = await getBlocks(request, library);
  const block = tree.blocks.find((candidate) => candidate.text.includes(text));
  if (!block) {
    throw new Error(`Block not found for ${text}`);
  }
  return block;
}

function rangeOf(block: BlockNodePayload, quote: string): { start: number; end: number } {
  const start = block.text.indexOf(quote);
  if (start === -1) {
    throw new Error(`Quote ${quote} not found in block ${block.block_id}`);
  }
  return { start, end: start + quote.length };
}

async function postTransaction(
  request: APIRequestContext,
  library: string,
  clientTxId: string,
  ops: Array<Record<string, unknown>>
): Promise<BlockTransactionAck> {
  const response = await request.post(`${documentApiUrl(library)}/transactions`, {
    data: {
      client_tx_id: clientTxId,
      actor: { kind: 'agent', id: 'ai:codex:smoke', label: 'Codex' },
      ops,
    },
  });
  await expectOk(response, `transaction ${clientTxId}`);
  return (await response.json()) as BlockTransactionAck;
}

async function getReview(
  request: APIRequestContext,
  library: string,
  includeResolved = false
): Promise<ReviewResponse> {
  const query = includeResolved ? '?includeResolved=1' : '';
  const response = await request.get(`${documentApiUrl(library)}/review${query}`);
  await expectOk(response, 'get review');
  return (await response.json()) as ReviewResponse;
}

async function waitForReview(
  request: APIRequestContext,
  library: string,
  matches: (review: ReviewResponse) => boolean,
  includeResolved = false,
  timeout = 20_000
): Promise<ReviewResponse> {
  let latest: ReviewResponse = { comments: [], suggestions: [] };
  await expect
    .poll(
      async () => {
        latest = await getReview(request, library, includeResolved);
        return matches(latest);
      },
      { timeout }
    )
    .toBe(true);
  return latest;
}

async function waitForPersistedMarkdown(
  request: APIRequestContext,
  library: string,
  text: string,
  timeout = 20_000
) {
  await expect
    .poll(async () => {
      const response = await request.get(documentApiUrl(library));
      if (!response.ok()) return '';
      return response.text();
    }, { timeout })
    .toContain(text);
}

async function expectOk(
  response: { ok(): boolean; status(): number; text(): Promise<string> },
  label: string
) {
  if (response.ok()) return;
  throw new Error(`${label} failed with ${response.status()}: ${await response.text()}`);
}

async function expectSaved(page: Page) {
  await expect(page.locator('[aria-label="Save status"]')).toContainText('Saved', {
    timeout: 20_000,
  });
}

async function expectNoConflictUi(page: Page) {
  await expect(page.getByText('External version available')).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Local draft' })).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Latest remote' })).toHaveCount(0);
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
