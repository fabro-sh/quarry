// Phase Zero, Gate B: server-as-collaborator spike.
//
// Proves that a third Yjs client (the future semantic mutation gateway) can
// mutate a live collab session concurrently with two typing humans, riding the
// same websocket update path as keystrokes — no flush/reseed bridge, no
// injection gate. The "agent" here is a Node-side y-websocket client (see
// tests/helpers/agent-collaborator.ts) connected to the same
// `/v1/collab/{documentId}` room as the browsers.
//
// Harness helpers (seedDocument, openHumanDocument, setCaretAfterText,
// expectSaved, expectNoConflictUi, documentApiUrl, waitForPersistedMarkdown)
// are duplicated from live-collab-agent-smoke.spec.ts, which does not export
// them; the copies are trimmed to what this spike needs.

import { expect, test, type APIRequestContext, type Browser, type Page } from 'playwright/test';

import { AgentCollaborator } from './helpers/agent-collaborator';

const LIVE_API_PORT = process.env.QUARRY_LIVE_API_PORT ?? '7832';
const API_ORIGIN = `http://127.0.0.1:${LIVE_API_PORT}`;
const COLLAB_WS_BASE = `ws://127.0.0.1:${LIVE_API_PORT}/v1/collab`;
const DOCUMENT_PATH = 'spike.md';

// Disjoint typing alphabets so per-browser keystroke sequences can be
// recovered exactly from the merged paragraph (the seed text contains no
// digits). Yjs preserves each client's own insertion order, so filtering the
// merged text down to one alphabet must reproduce that client's typed string.
const A_ALPHABET = '01234';
const B_ALPHABET = '56789';

const AGENT_REWRITE = 'Agent rewrote the target block mid-typing.';
const SAME_BLOCK_REWRITE = 'Agent replaced the whole paragraph.';
const LONG_REWRITE =
  'Agent rewrote this block with a much longer body so everything below it ' +
  'reflows to a different screen position when the update lands. '.repeat(2) +
  'End of rewrite.';

test.describe.configure({ timeout: 120_000 });

test('agent collaborator edits a different block while two humans type; undo stays isolated', async ({
  browser,
  request,
}, testInfo) => {
  const library = uniqueLibrary('spike-diff', testInfo.workerIndex);
  await seedDocument(request, library, [
    '# Collaborator spike',
    '',
    'Humans type here.',
    '',
    'Agent target block.',
    '',
    'Trailing stability block.',
    '',
  ]);

  const userA = await openHumanDocument(browser, library, 'Avery');
  const userB = await openHumanDocument(browser, library, 'Blair');
  let agent: AgentCollaborator | undefined;
  try {
    await expect(editorOf(userA.page)).toContainText('Agent target block.');
    await expect(editorOf(userB.page)).toContainText('Agent target block.');

    agent = await connectAgent(request, library);
    const typingBlock = agent.findBlockIndex('Humans type here.');
    const targetBlock = agent.findBlockIndex('Agent target block.');

    // The agent shares the room's awareness with both browsers under its own
    // distinct client ID (a separate Y.Doc allocates it automatically).
    const liveAgent = agent;
    agent.queryAwareness();
    await expect
      .poll(() => liveAgent.remoteClientIds().length, { timeout: 15_000 })
      .toBeGreaterThanOrEqual(2);

    await stampEditorDom(userA.page);
    await stampEditorDom(userB.page);

    await setCaretAfterText(userA.page, 'Humans type here.');
    await setCaretAfterText(userB.page, 'Humans type');

    const aTyped = A_ALPHABET.repeat(4);
    const bTyped = B_ALPHABET.repeat(4);
    const typingA = userA.page.keyboard.type(aTyped, { delay: 70 });
    const typingB = userB.page.keyboard.type(bTyped, { delay: 70 });

    // Mutate while keystrokes are still in flight: wait until a few digits
    // from each browser have reached the agent's replica, then replace the
    // other block in a single tagged transaction.
    await expect
      .poll(
        () => {
          const text = liveAgent.blockTextAt(typingBlock);
          return Math.min(
            onlyChars(text, A_ALPHABET).length,
            onlyChars(text, B_ALPHABET).length
          );
        },
        { intervals: [25], timeout: 10_000 }
      )
      .toBeGreaterThanOrEqual(2);
    const digitsSeenAtInjection = onlyChars(
      liveAgent.blockTextAt(typingBlock),
      A_ALPHABET + B_ALPHABET
    ).length;
    agent.replaceBlockContent(targetBlock, AGENT_REWRITE);

    await typingA;
    await typingB;
    expect(digitsSeenAtInjection, 'replacement landed mid-typing').toBeLessThan(
      aTyped.length + bTyped.length
    );

    await expectBrowsersConverged(userA.page, userB.page);
    await expectAgentConverged(userA.page, agent);

    const blocksA = await pageBlockTexts(userA.page);
    const blocksB = await pageBlockTexts(userB.page);
    const merged = blocksA[typingBlock];
    expect(blocksB[typingBlock]).toBe(merged);
    // No in-flight keystroke was lost or reordered.
    expect(onlyChars(merged, A_ALPHABET)).toBe(aTyped);
    expect(onlyChars(merged, B_ALPHABET)).toBe(bTyped);
    expect(merged).toHaveLength('Humans type here.'.length + aTyped.length + bTyped.length);
    // The agent's replacement landed in the other block on both screens.
    expect(blocksA[targetBlock]).toBe(AGENT_REWRITE);
    expect(blocksB[targetBlock]).toBe(AGENT_REWRITE);

    // Receiving tabs settle to "Saved" — never dirty/failed/conflicted — the
    // checkpoint ack covers the agent's edit exactly like a human keystroke.
    await expectSaved(userA.page);
    await expectSaved(userB.page);
    await expectNoConflictUi(userA.page);
    await expectNoConflictUi(userB.page);
    await waitForPersistedMarkdown(request, library, AGENT_REWRITE);
    // No remount/reseed: the stamped editor DOM node survived the injection.
    await expectEditorNotRemounted(userA.page);
    await expectEditorNotRemounted(userB.page);

    // Undo isolation: A's undo removes only A's keystrokes; B's text and the
    // agent's replacement stand, in both browsers.
    await undoUntilGone(userA.page, typingBlock, A_ALPHABET);
    await expectBrowsersConverged(userA.page, userB.page);
    const afterUndoA = await pageBlockTexts(userB.page);
    expect(onlyChars(afterUndoA[typingBlock], A_ALPHABET)).toBe('');
    expect(onlyChars(afterUndoA[typingBlock], B_ALPHABET)).toBe(bTyped);
    expect(afterUndoA[targetBlock]).toBe(AGENT_REWRITE);

    // Same for B's undo: only B's keystrokes disappear.
    await undoUntilGone(userB.page, typingBlock, B_ALPHABET);
    await expectBrowsersConverged(userA.page, userB.page);
    const afterUndoB = await pageBlockTexts(userA.page);
    expect(afterUndoB[typingBlock]).toBe('Humans type here.');
    expect(afterUndoB[targetBlock]).toBe(AGENT_REWRITE);
    await expectAgentConverged(userA.page, agent);

    expect(userA.pageErrors).toEqual([]);
    expect(userB.pageErrors).toEqual([]);
  } finally {
    agent?.destroy();
    await userA.context.close();
    await userB.context.close();
  }
});

test('agent collaborator rewrites the block a human is typing in, without rejection', async ({
  browser,
  request,
}, testInfo) => {
  const library = uniqueLibrary('spike-same', testInfo.workerIndex);
  await seedDocument(request, library, [
    '# Same block spike',
    '',
    'Same block typing target.',
    '',
    'Trailing stability block.',
    '',
  ]);

  const userA = await openHumanDocument(browser, library, 'Avery');
  const userB = await openHumanDocument(browser, library, 'Blair');
  let agent: AgentCollaborator | undefined;
  try {
    await expect(editorOf(userA.page)).toContainText('Same block typing target.');
    await expect(editorOf(userB.page)).toContainText('Same block typing target.');

    agent = await connectAgent(request, library);
    const block = agent.findBlockIndex('Same block typing target.');

    await stampEditorDom(userA.page);
    await stampEditorDom(userB.page);

    await setCaretAfterText(userA.page, 'Same block typing target.');
    const head = A_ALPHABET.repeat(3);
    const liveAgent = agent;
    const typing = userA.page.keyboard.type(head, { delay: 70 });
    await expect
      .poll(() => onlyChars(liveAgent.blockTextAt(block), A_ALPHABET).length, {
        intervals: [25],
        timeout: 10_000,
      })
      .toBeGreaterThanOrEqual(2);
    const digitsSeenAtInjection = onlyChars(liveAgent.blockTextAt(block), A_ALPHABET).length;
    agent.replaceBlockContent(block, SAME_BLOCK_REWRITE);
    await typing;
    expect(digitsSeenAtInjection, 'replacement landed mid-typing').toBeLessThan(head.length);

    // The rewrite lands in the very block A is typing in: no rejection, no
    // conflict UI — the editors just merge and stay editable.
    await expect(editorOf(userA.page)).toContainText(SAME_BLOCK_REWRITE);
    await expect(editorOf(userB.page)).toContainText(SAME_BLOCK_REWRITE);
    await setCaretAfterText(userA.page, 'whole paragraph.');
    const tail = B_ALPHABET;
    await userA.page.keyboard.type(tail, { delay: 40 });

    await expectBrowsersConverged(userA.page, userB.page);
    await expectAgentConverged(userA.page, agent);

    const blocksA = await pageBlockTexts(userA.page);
    const blocksB = await pageBlockTexts(userB.page);
    const merged = blocksA[block];
    expect(blocksB[block]).toBe(merged);
    // Convergence without CRDT loss: the agent's rewrite survives contiguously
    // (it deleted every non-digit character it had seen, including the seed
    // text), and A's post-rewrite keystrokes land exactly where A typed them.
    expect(withoutDigits(merged)).toBe(SAME_BLOCK_REWRITE);
    expect(merged).toContain(`whole paragraph.${tail}`);
    expect(onlyChars(merged, B_ALPHABET)).toBe(tail);
    // Pre-rewrite keystrokes that outran the agent's snapshot merge in as a
    // contiguous suffix of what A typed (awkward merged text is expected).
    const survivors = onlyChars(merged, A_ALPHABET);
    expect(head.endsWith(survivors)).toBe(true);

    await expectSaved(userA.page);
    await expectSaved(userB.page);
    await expectNoConflictUi(userA.page);
    await expectNoConflictUi(userB.page);
    await expectEditorNotRemounted(userA.page);
    await expectEditorNotRemounted(userB.page);
    expect(userA.pageErrors).toEqual([]);
    expect(userB.pageErrors).toEqual([]);
  } finally {
    agent?.destroy();
    await userA.context.close();
    await userB.context.close();
  }
});

test('remote cursor stays anchored to its character across an agent edit above it', async ({
  browser,
  request,
}, testInfo) => {
  const library = uniqueLibrary('spike-aware', testInfo.workerIndex);
  await seedDocument(request, library, [
    '# Awareness spike',
    '',
    'Agent reflow target block.',
    '',
    'Cursor anchor paragraph for Blair.',
    '',
  ]);

  const userA = await openHumanDocument(browser, library, 'Avery');
  const userB = await openHumanDocument(browser, library, 'Blair');
  let agent: AgentCollaborator | undefined;
  try {
    await expect(editorOf(userA.page)).toContainText('Cursor anchor paragraph for Blair.');
    await expect(editorOf(userB.page)).toContainText('Cursor anchor paragraph for Blair.');

    agent = await connectAgent(request, library);
    const reflowBlock = agent.findBlockIndex('Agent reflow target block.');

    // B parks its caret right after the word "anchor"; the type+backspace
    // nudge makes Slate adopt the programmatic DOM selection (the editor
    // auto-selects the document end on init) and broadcast it via awareness.
    // A renders it as a labelled remote caret in the RemoteCursorOverlay.
    await setCaretAfterText(userB.page, 'Cursor anchor');
    await userB.page.keyboard.type('x');
    await userB.page.keyboard.press('Backspace');
    const remoteCaret = editorOf(userA.page)
      .locator('xpath=..')
      .locator('div[aria-hidden="true"]', { hasText: 'Blair' });
    await expect(remoteCaret).toBeVisible({ timeout: 15_000 });

    const anchorBefore = await anchorCharRect(userA.page);
    await expect
      .poll(
        async () => {
          const anchor = await anchorCharRect(userA.page);
          const caret = await remoteCaret.boundingBox();
          if (!caret) return Number.POSITIVE_INFINITY;
          return Math.max(Math.abs(caret.x - anchor.right), Math.abs(caret.y - anchor.top));
        },
        { timeout: 10_000 }
      )
      .toBeLessThanOrEqual(6);

    // The agent rewrites the block above; the anchor paragraph reflows to a
    // new screen position…
    agent.replaceBlockContent(reflowBlock, LONG_REWRITE);
    await expect(editorOf(userA.page)).toContainText('End of rewrite.');
    await expect(editorOf(userB.page)).toContainText('End of rewrite.');
    await expect
      .poll(async () => Math.abs((await anchorCharRect(userA.page)).top - anchorBefore.top), {
        timeout: 10_000,
      })
      .toBeGreaterThan(10);

    // …and B's remote caret tracks the same logical character to its new
    // position instead of staying at stale screen coordinates.
    await expect
      .poll(
        async () => {
          const anchor = await anchorCharRect(userA.page);
          const caret = await remoteCaret.boundingBox();
          if (!caret) return Number.POSITIVE_INFINITY;
          return Math.max(Math.abs(caret.x - anchor.right), Math.abs(caret.y - anchor.top));
        },
        { timeout: 10_000 }
      )
      .toBeLessThanOrEqual(6);

    // B's own selection is still collapsed right after "anchor".
    const selection = await userB.page.evaluate(() => {
      const sel = window.getSelection();
      if (!sel || !sel.anchorNode) return null;
      return {
        collapsed: sel.isCollapsed,
        offset: sel.anchorOffset,
        text: sel.anchorNode.textContent ?? '',
      };
    });
    expect(selection?.collapsed).toBe(true);
    expect(selection?.text).toContain('Cursor anchor paragraph for Blair.');
    expect(selection?.offset).toBe('Cursor anchor'.length);

    await expectBrowsersConverged(userA.page, userB.page);
    await expectNoConflictUi(userA.page);
    await expectNoConflictUi(userB.page);
    expect(userA.pageErrors).toEqual([]);
    expect(userB.pageErrors).toEqual([]);
  } finally {
    agent?.destroy();
    await userA.context.close();
    await userB.context.close();
  }
});

test('remote cursor stays anchored to its character across an agent edit in the same block', async ({
  browser,
  request,
}, testInfo) => {
  const library = uniqueLibrary('spike-sameblock', testInfo.workerIndex);
  await seedDocument(request, library, [
    '# Same-block cursor spike',
    '',
    'Cursor anchor paragraph for Blair. The agent will rewrite this tail shortly.',
    '',
  ]);

  const userA = await openHumanDocument(browser, library, 'Avery');
  const userB = await openHumanDocument(browser, library, 'Blair');
  try {
    await expect(editorOf(userA.page)).toContainText('rewrite this tail shortly.');
    await expect(editorOf(userB.page)).toContainText('rewrite this tail shortly.');

    await setCaretAfterText(userB.page, 'Cursor anchor');
    await userB.page.keyboard.type('x');
    await userB.page.keyboard.press('Backspace');
    const remoteCaret = editorOf(userA.page)
      .locator('xpath=..')
      .locator('div[aria-hidden="true"]', { hasText: 'Blair' });
    await expect(remoteCaret).toBeVisible({ timeout: 15_000 });
    await expect
      .poll(
        async () => {
          const anchor = await anchorCharRect(userA.page);
          const caret = await remoteCaret.boundingBox();
          if (!caret) return Number.POSITIVE_INFINITY;
          return Math.max(Math.abs(caret.x - anchor.right), Math.abs(caret.y - anchor.top));
        },
        { timeout: 10_000 }
      )
      .toBeLessThanOrEqual(6);

    // The agent rewrites the tail of the very block B's caret sits in,
    // through the GATEWAY (the real agent surface): the session reconciler
    // splices only the changed span, so the anchored prefix's Yjs items —
    // and with them B's cursor — survive. (The old wholesale rewrite deleted
    // every item in the block and threw the caret to its start.)
    const blocksResponse = await request.get(`${documentApiUrl(library)}/blocks`);
    expect(blocksResponse.ok()).toBeTruthy();
    const tree = (await blocksResponse.json()) as {
      blocks: Array<{ block_id: string; text: string }>;
    };
    const target = tree.blocks.find((candidate) =>
      candidate.text.includes('Cursor anchor paragraph for Blair.')
    );
    expect(target).toBeTruthy();
    const transaction = await request.post(`${documentApiUrl(library)}/transactions`, {
      data: {
        client_tx_id: 'tx-spike-same-block',
        actor: { kind: 'agent', id: 'ai:codex:spike', label: 'Codex' },
        ops: [
          {
            op: 'replace_block_content',
            block_id: target!.block_id,
            text: 'Cursor anchor paragraph for Blair. A freshly spliced tail took its place.',
          },
        ],
      },
    });
    expect(transaction.ok()).toBeTruthy();
    await expect(editorOf(userA.page)).toContainText('freshly spliced tail');
    await expect(editorOf(userB.page)).toContainText('freshly spliced tail');

    // B's remote caret still hugs the same logical character…
    await expect
      .poll(
        async () => {
          const anchor = await anchorCharRect(userA.page);
          const caret = await remoteCaret.boundingBox();
          if (!caret) return Number.POSITIVE_INFINITY;
          return Math.max(Math.abs(caret.x - anchor.right), Math.abs(caret.y - anchor.top));
        },
        { timeout: 10_000 }
      )
      .toBeLessThanOrEqual(6);

    // …and B's own selection is still collapsed right after "anchor".
    const selection = await userB.page.evaluate(() => {
      const sel = window.getSelection();
      if (!sel || !sel.anchorNode) return null;
      return {
        collapsed: sel.isCollapsed,
        offset: sel.anchorOffset,
        text: sel.anchorNode.textContent ?? '',
      };
    });
    expect(selection?.collapsed).toBe(true);
    expect(selection?.text).toContain('Cursor anchor paragraph for Blair.');
    expect(selection?.offset).toBe('Cursor anchor'.length);

    await expectBrowsersConverged(userA.page, userB.page);
    await expectNoConflictUi(userA.page);
    await expectNoConflictUi(userB.page);
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

function onlyChars(text: string, alphabet: string): string {
  return [...text].filter((char) => alphabet.includes(char)).join('');
}

function withoutDigits(text: string): string {
  return text.replace(/[0-9]/g, '');
}

function normalize(text: string): string {
  // Slate renders zero-width placeholders inside empty text nodes.
  return text.replace(/[\uFEFF\u200B]/g, '');
}

async function connectAgent(request: APIRequestContext, library: string) {
  const response = await request.get(documentApiUrl(library));
  expect(response.ok()).toBeTruthy();
  const documentId = response.headers()['x-quarry-document-id'];
  if (!documentId) throw new Error('document id header missing');
  const agent = new AgentCollaborator(COLLAB_WS_BASE, documentId);
  await agent.whenSynced();
  await expect.poll(() => agent.blockTexts().length).toBeGreaterThan(0);
  return agent;
}

async function editorText(page: Page): Promise<string> {
  return normalize((await editorOf(page).textContent()) ?? '');
}

async function expectBrowsersConverged(pageA: Page, pageB: Page) {
  await expect
    .poll(
      async () => {
        const [textA, textB] = await Promise.all([editorText(pageA), editorText(pageB)]);
        return textA === textB ? 'converged' : `A=${textA} || B=${textB}`;
      },
      { timeout: 20_000 }
    )
    .toBe('converged');
}

async function expectAgentConverged(page: Page, agent: AgentCollaborator) {
  await expect
    .poll(
      async () => {
        const pageBlocks = (await pageBlockTexts(page)).join('\n');
        const agentBlocks = agent.blockTexts().map(normalize).join('\n');
        return pageBlocks === agentBlocks
          ? 'converged'
          : `page=${pageBlocks} || agent=${agentBlocks}`;
      },
      { timeout: 20_000 }
    )
    .toBe('converged');
}

async function pageBlockTexts(page: Page): Promise<string[]> {
  const blocks = await page.evaluate(() => {
    const editor = document.querySelector('[aria-label="Plate markdown editor"]');
    if (!editor) throw new Error('Plate markdown editor not found');
    return Array.from(editor.querySelectorAll('[data-slate-node="element"]'))
      .filter((element) => !element.parentElement?.closest('[data-slate-node="element"]'))
      .map((element) => element.textContent ?? '');
  });
  return blocks.map(normalize);
}

async function undoUntilGone(page: Page, blockIndex: number, alphabet: string) {
  await editorOf(page).focus();
  for (let press = 0; press < 15; press += 1) {
    const blocks = await pageBlockTexts(page);
    if (onlyChars(blocks[blockIndex] ?? '', alphabet) === '') return;
    await page.keyboard.press('ControlOrMeta+z');
  }
  throw new Error(`undo never removed "${alphabet}" characters from block ${blockIndex}`);
}

async function stampEditorDom(page: Page) {
  await editorOf(page).evaluate((element) => {
    element.setAttribute('data-spike-epoch', 'original');
  });
}

async function expectEditorNotRemounted(page: Page) {
  await expect(
    page.locator('[aria-label="Plate markdown editor"][data-spike-epoch="original"]')
  ).toHaveCount(1);
}

async function anchorCharRect(page: Page) {
  return page.evaluate(() => {
    const editor = document.querySelector('[aria-label="Plate markdown editor"]');
    if (!editor) throw new Error('Plate markdown editor not found');
    const walker = document.createTreeWalker(editor, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const content = node.textContent ?? '';
      const index = content.indexOf('Cursor anchor');
      if (index === -1) continue;
      const offset = index + 'Cursor anchor'.length;
      const range = document.createRange();
      range.setStart(node, offset - 1);
      range.setEnd(node, offset);
      const rect = range.getBoundingClientRect();
      return { right: rect.right, top: rect.top };
    }
    throw new Error('anchor text not found');
  });
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

function documentApiUrl(library: string) {
  return `${API_ORIGIN}/v1/libraries/${encodeURIComponent(library)}/documents/${encodeURIComponent(
    DOCUMENT_PATH
  )}`;
}
