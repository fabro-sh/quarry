import { expect, test, type APIRequestContext, type Browser, type Page } from 'playwright/test';

interface AgentBlockRef {
  ordinal: number;
  contentHash: string;
}

interface AgentSnapshotBlock {
  ref: AgentBlockRef;
  markdown: string;
}

interface AgentDocumentSnapshot {
  documentId: string;
  baseToken: string;
  blocks: AgentSnapshotBlock[];
}

interface AgentMutationResponse {
  dryRun: boolean;
  injection?: string | null;
  nextBaseToken?: string | null;
  results?: Array<{ id?: string | null; op: string }>;
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

    await setCaretAfterText(userA.page, 'Human direct block target.');
    await userA.page.keyboard.type(' from User A');

    await expect(editorB).toContainText('Human direct block target. from User A');
    await expectSaved(userA.page);
    await expectSaved(userB.page);
    await waitForPersistedMarkdown(request, library, 'from User A');
    await expectNoConflictUi(userA.page);
    await expectNoConflictUi(userB.page);

    const modeB = userB.page.getByRole('button', { name: 'Document mode' });
    await modeB.click();
    await userB.page.getByRole('menuitem', { name: 'Suggesting' }).click();
    await expect(modeB).toContainText('Suggesting');

    await setCaretAfterText(userB.page, 'Human suggesting block target.');
    await userB.page.keyboard.type('User B suggestion');

    await expect(editorA).toContainText('User B suggestion');
    await expect(
      userA.page.getByTestId('suggestion-card').filter({ hasText: 'User B suggestion' })
    ).toBeVisible();
    await expectSaved(userA.page);
    await expectSaved(userB.page);
    await waitForPersistedMarkdown(request, library, 'User B suggestion');
    await expectNoConflictUi(userA.page);
    await expectNoConflictUi(userB.page);

    await userA.page.getByRole('button', { name: 'Add agent' }).click();
    const addAgentDialog = userA.page.getByRole('dialog', { name: 'Add agent' });
    await expect(addAgentDialog).toContainText(`/lib/${library}/documents/${DOCUMENT_PATH}`);
    for (const endpoint of ['/presence', '/snapshot', '/edit', '/ops']) {
      await expect(addAgentDialog).toContainText(
        `/v1/libraries/${library}/documents/${DOCUMENT_PATH}${endpoint}`
      );
    }
    await userA.page.getByRole('button', { name: 'Close' }).click();

    const presence = await request.post(`${documentApiUrl(library)}/presence`, {
      data: { status: 'reading', by: 'Codex' },
      headers: { 'X-Agent-Id': 'ai:codex:smoke' },
    });
    expect(presence.ok()).toBeTruthy();
    await expect(userA.page.getByLabel(/Codex .* reading/)).toBeVisible();
    await expectNoConflictUi(userA.page);
    await expectNoConflictUi(userB.page);

    let snapshot = await getSnapshot(request, library);
    const directBlock = blockContaining(snapshot, 'Agent direct block target.');
    const edit = await postAgentEdit(request, library, {
      baseToken: snapshot.baseToken,
      operations: [
        {
          op: 'replace_block',
          ref: directBlock.ref,
          block: { markdown: 'Agent direct block target. Agent REST edit landed.\n\n' },
        },
      ],
    });
    expect(edit.injection).toBe('injected');

    await expect(editorA).toContainText('Agent REST edit landed');
    await expect(editorB).toContainText('Agent REST edit landed');
    await expectNoConflictUi(userA.page);
    await expectNoConflictUi(userB.page);

    const comment = await postAgentOpsFromSnapshot(request, library, 'Codex', (latest) => {
      const commentBlock = blockContaining(latest, 'Agent comment target appears here.');
      return [
        {
          op: 'comment.add',
          id: 'agent-comment-smoke',
          ref: commentBlock.ref,
          quote: 'comment target',
          body: 'Agent comment landed.',
        },
      ];
    });
    expect(comment.injection).toBe('injected');
    expect(comment.results?.[0]?.id).toBe('agent-comment-smoke');

    await expect(
      userA.page.getByTestId('comment-card').filter({ hasText: 'Agent comment landed.' })
    ).toBeVisible();
    await expect(
      userB.page.getByTestId('comment-card').filter({ hasText: 'Agent comment landed.' })
    ).toBeVisible();
    await expect(editorA.locator('[data-comment-id="agent-comment-smoke"]')).toBeVisible();
    await expect(editorB.locator('[data-comment-id="agent-comment-smoke"]')).toBeVisible();
    await expectSaved(userA.page);
    await expectSaved(userB.page);
    await waitForStableSnapshot(request, library);
    await expectNoConflictUi(userA.page);
    await expectNoConflictUi(userB.page);

    const suggestion = await postAgentOpsFromSnapshot(request, library, 'Codex', (latest) => {
      const suggestionBlock = blockContaining(latest, 'Review suggestion target here.');
      return [
        {
          op: 'suggestion.add',
          id: 'agent-suggestion-smoke',
          kind: 'substitution',
          ref: suggestionBlock.ref,
          quote: 'suggestion target',
          content: 'agent suggestion replacement',
        },
      ];
    });
    expect(suggestion.injection).toBe('injected');
    expect(suggestion.results?.[0]?.id).toBe('agent-suggestion-smoke');
    await waitForPersistedMarkdown(request, library, 'agent-suggestion-smoke');
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
      editorA.locator('[data-suggestion-id="agent-suggestion-smoke"]').first()
    ).toBeVisible();
    await expect(
      editorB.locator('[data-suggestion-id="agent-suggestion-smoke"]').first()
    ).toBeVisible();
    await expectNoConflictUi(userA.page);
    await expectNoConflictUi(userB.page);

    await agentSuggestionA.hover();
    await agentSuggestionA.getByTestId('rail-accept').click();

    await expect(agentSuggestionA).toHaveCount(0);
    await expect(agentSuggestionB).toHaveCount(0);
    await expect(editorA).toContainText('Review agent suggestion replacement here.');
    await expect(editorB).toContainText('Review agent suggestion replacement here.');
    await expectSaved(userA.page);
    await expectSaved(userB.page);
    await waitForPersistedMarkdown(
      request,
      library,
      'Review agent suggestion replacement here.',
      60_000
    );
    await expect
      .poll(async () => readPersistedMarkdown(request, library), { timeout: 60_000 })
      .not.toContain('agent-suggestion-smoke');
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
  return { context, page };
}

async function getSnapshot(request: APIRequestContext, library: string) {
  const response = await request.get(`${documentApiUrl(library)}/snapshot`);
  expect(response.ok()).toBeTruthy();
  return (await response.json()) as AgentDocumentSnapshot;
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

async function readPersistedMarkdown(request: APIRequestContext, library: string) {
  const response = await request.get(documentApiUrl(library));
  if (!response.ok()) return '';
  return response.text();
}

async function waitForStableSnapshot(request: APIRequestContext, library: string) {
  let previous = await getSnapshot(request, library);
  for (let attempt = 0; attempt < 20; attempt += 1) {
    await new Promise((resolve) => setTimeout(resolve, 500));
    const next = await getSnapshot(request, library);
    if (next.baseToken === previous.baseToken) return next;
    previous = next;
  }
  throw new Error('Document snapshot did not stabilize');
}

async function postAgentEdit(
  request: APIRequestContext,
  library: string,
  body: Record<string, unknown>
) {
  const response = await request.post(`${documentApiUrl(library)}/edit`, { data: body });
  await expectOk(response, 'agent edit');
  return (await response.json()) as AgentMutationResponse;
}

async function postAgentOpsFromSnapshot(
  request: APIRequestContext,
  library: string,
  by: string,
  buildOperations: (snapshot: AgentDocumentSnapshot) => Array<Record<string, unknown>>
) {
  let stale = '';
  for (let attempt = 0; attempt < 3; attempt += 1) {
    const snapshot = await getSnapshot(request, library);
    const operations = buildOperations(snapshot);
    const response = await request.post(`${documentApiUrl(library)}/ops`, {
      data: {
        baseToken: snapshot.baseToken,
        by,
        operations,
      },
    });
    if (response.ok()) return (await response.json()) as AgentMutationResponse;

    const body = await response.text();
    if (response.status() === 412 && body.includes('STALE_BASE')) {
      stale = body;
      continue;
    }
    throw new Error(`agent ops failed with ${response.status()}: ${body}`);
  }
  throw new Error(`agent ops stayed stale after retrying: ${stale}`);
}

async function expectOk(
  response: { ok(): boolean; status(): number; text(): Promise<string> },
  label: string
) {
  if (response.ok()) return;
  throw new Error(`${label} failed with ${response.status()}: ${await response.text()}`);
}

function blockContaining(snapshot: AgentDocumentSnapshot, text: string) {
  const block = snapshot.blocks.find((candidate) => candidate.markdown.includes(text));
  if (!block) {
    throw new Error(`Snapshot block not found for ${text}`);
  }
  return block;
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
