import {
  expect,
  test,
  type APIRequestContext,
  type Browser,
  type BrowserContext,
  type Page,
} from 'playwright/test';

import blockCapabilities from '../../crates/quarry-collab-codec/block-capabilities.json' with {
  type: 'json',
};
import {
  BLOCK_DELETE_CASES,
  BLOCK_DELETE_FIXTURE_MARKDOWN,
  BLOCK_DELETE_PRESERVED_MARKERS,
  assertBlockDeleteRegistryCoverage,
  interleavedBlockDeleteCases,
  type BlockDeleteConformanceCase,
} from '../src/features/review/__fixtures__/block-delete-conformance';

// This is the only suite that drives the complete server rows -> Yjs session
// -> Plate resolution -> server checkpoint path. The fast companion suite runs
// the same cases through several acceptance orders on every ordinary UI test.

interface BlockNodePayload {
  attrs: Record<string, unknown>;
  block_id: string;
  block_type: string;
  parent_block_id: string | null;
  position: number;
  text: string;
}

interface BlockTreeResponse {
  blocks: BlockNodePayload[];
  document_clock: string;
  document_id: string;
}

interface BlockTransactionAck {
  changed_block_ids: string[];
  document_clock: string;
  status: string;
  transaction_id: string;
}

interface ReviewItem {
  body?: string;
  id: string;
  kind: string;
  status: string;
}

interface ReviewResponse {
  comments: ReviewItem[];
  suggestions: ReviewItem[];
}

interface PreparedDocument {
  baselineMarkdown: string;
  documentUrl: string;
  targetSubtrees: Map<string, string[]>;
}

const API_ORIGIN = `http://127.0.0.1:${process.env.QUARRY_LIVE_API_PORT ?? '7832'}`;
const KNOWN_BLOCK_TYPES = blockCapabilities.map((entry) => entry.type);

test.describe.configure({ timeout: 240_000 });

test('accepting registered block deletions removes exact subtrees without leftovers', async ({
  browser,
  request,
}, testInfo) => {
  assertBlockDeleteRegistryCoverage(KNOWN_BLOCK_TYPES);
  const library = uniqueLibrary('accept', testInfo.workerIndex);
  const path = 'accept.md';
  const prepared = await prepareDocument(request, library, path);
  const human = await openHumanDocument(browser, library, path);

  try {
    await expectAllSuggestionsInEditor(human.page);
    for (const fixture of interleavedBlockDeleteCases()) {
      await resolveEditorSuggestion(human.page, fixture, 'accept');
    }
    await expect(human.page.getByTestId('suggestion-card')).toHaveCount(0);
    await expectSaved(human.page);
    await waitForNoOpenSuggestions(request, prepared.documentUrl);

    const finalTree = await waitForTargetSubtrees(
      request,
      prepared.documentUrl,
      prepared.targetSubtrees,
      false
    );
    assertPreservedContent(finalTree);
    assertNoStructuralLeftovers(finalTree);
    assertTargetSubtrees(finalTree, prepared.targetSubtrees, false);

    const finalMarkdown = await getMarkdown(request, prepared.documentUrl);
    for (const fixture of BLOCK_DELETE_CASES) {
      if (fixture.marker) expect(finalMarkdown).not.toContain(fixture.marker);
    }
    await expectCanonicalServerRoundTrip(request, library, 'accept-roundtrip.md', finalMarkdown);
  } finally {
    await human.context.close();
  }
});

test('rejecting registered block deletions preserves every target and its serialization', async ({
  browser,
  request,
}, testInfo) => {
  assertBlockDeleteRegistryCoverage(KNOWN_BLOCK_TYPES);
  const library = uniqueLibrary('reject', testInfo.workerIndex);
  const path = 'reject.md';
  const prepared = await prepareDocument(request, library, path);
  const human = await openHumanDocument(browser, library, path);

  try {
    await expectAllSuggestionsInEditor(human.page);
    for (const fixture of [...BLOCK_DELETE_CASES].reverse()) {
      await resolveEditorSuggestion(human.page, fixture, 'reject');
    }
    await expect(human.page.getByTestId('suggestion-card')).toHaveCount(0);
    await expectSaved(human.page);
    await waitForNoOpenSuggestions(request, prepared.documentUrl);

    const finalTree = await waitForTargetSubtrees(
      request,
      prepared.documentUrl,
      prepared.targetSubtrees,
      true
    );
    assertPreservedContent(finalTree);
    assertNoStructuralLeftovers(finalTree);
    assertTargetSubtrees(finalTree, prepared.targetSubtrees, true);

    const finalMarkdown = await getMarkdown(request, prepared.documentUrl);
    expect(finalMarkdown).toBe(prepared.baselineMarkdown);
    await expectCanonicalServerRoundTrip(request, library, 'reject-roundtrip.md', finalMarkdown);
  } finally {
    await human.context.close();
  }
});

async function prepareDocument(
  request: APIRequestContext,
  library: string,
  path: string
): Promise<PreparedDocument> {
  const libraryResponse = await request.post(`${API_ORIGIN}/v1/libraries`, {
    data: { slug: library },
  });
  expect(libraryResponse.status()).toBe(201);
  const documentUrl = documentApiUrl(library, path);
  await putMarkdown(request, documentUrl, BLOCK_DELETE_FIXTURE_MARKDOWN);
  const baselineMarkdown = await getMarkdown(request, documentUrl);
  const tree = await getBlocks(request, documentUrl);
  const targets = new Map(
    BLOCK_DELETE_CASES.map((fixture) => [fixture.name, targetBlock(tree, fixture)])
  );
  const targetSubtrees = new Map(
    BLOCK_DELETE_CASES.map((fixture) => {
      const target = targets.get(fixture.name);
      if (!target) throw new Error(`missing target for ${fixture.name}`);
      return [fixture.name, subtreeIds(tree, target.block_id)];
    })
  );

  const transaction = await request.post(`${documentUrl}/transactions`, {
    data: {
      actor: { kind: 'agent', id: 'ai:conformance', label: 'Conformance' },
      client_tx_id: `suggest-block-deletes-${crypto.randomUUID()}`,
      ops: BLOCK_DELETE_CASES.map((fixture) => ({
        op: 'suggestion.add_block_delete',
        block_id: targets.get(fixture.name)?.block_id,
        body: suggestionBody(fixture),
      })),
    },
  });
  await expectOk(transaction, 'add block-delete suggestions');
  const ack = (await transaction.json()) as BlockTransactionAck;
  expect(ack.status).toMatch(/^committed/);

  const review = await getReview(request, documentUrl);
  const suggestionIds = new Set<string>();
  for (const fixture of BLOCK_DELETE_CASES) {
    const suggestion = review.suggestions.find((item) => item.body === suggestionBody(fixture));
    if (!suggestion) throw new Error(`missing review suggestion for ${fixture.name}`);
    expect(suggestion.kind).toBe('block_delete');
    expect(suggestion.status).toBe('open');
    suggestionIds.add(suggestion.id);
  }
  expect(suggestionIds.size).toBe(BLOCK_DELETE_CASES.length);

  return { baselineMarkdown, documentUrl, targetSubtrees };
}

async function openHumanDocument(
  browser: Browser,
  library: string,
  path: string
): Promise<{ context: BrowserContext; page: Page }> {
  const context = await browser.newContext();
  await context.addInitScript(() => {
    window.localStorage.setItem('quarry:author', 'Conformance Reviewer');
    window.localStorage.setItem('quarry:theme', 'light');
  });
  const page = await context.newPage();
  await page.goto(
    `/lib/${encodeURIComponent(library)}/documents/${encodeURIComponent(path)}`
  );
  await expect(page.locator('[data-collab-save-state="saved"]')).toBeVisible({
    timeout: 30_000,
  });
  return { context, page };
}

async function expectAllSuggestionsInEditor(page: Page): Promise<void> {
  const cards = page.getByTestId('suggestion-card');
  await expect(cards).toHaveCount(BLOCK_DELETE_CASES.length, { timeout: 30_000 });
  for (const fixture of BLOCK_DELETE_CASES) {
    await expect(cards.filter({ hasText: suggestionBody(fixture) })).toHaveCount(1);
  }
}

async function resolveEditorSuggestion(
  page: Page,
  fixture: BlockDeleteConformanceCase,
  resolution: 'accept' | 'reject'
): Promise<void> {
  const card = page.getByTestId('suggestion-card').filter({ hasText: suggestionBody(fixture) });
  await card.scrollIntoViewIfNeeded();
  await card.hover();
  await card.getByTestId(resolution === 'accept' ? 'rail-accept' : 'rail-reject').click();
  await expect(card).toHaveCount(0);
}

async function waitForNoOpenSuggestions(
  request: APIRequestContext,
  documentUrl: string
): Promise<void> {
  await expect
    .poll(async () => (await getReview(request, documentUrl)).suggestions.length, {
      timeout: 60_000,
    })
    .toBe(0);
}

async function waitForTargetSubtrees(
  request: APIRequestContext,
  documentUrl: string,
  targetSubtrees: Map<string, string[]>,
  present: boolean
): Promise<BlockTreeResponse> {
  let latest = await getBlocks(request, documentUrl);
  await expect
    .poll(
      async () => {
        latest = await getBlocks(request, documentUrl);
        const ids = new Set(latest.blocks.map((block) => block.block_id));
        return [...targetSubtrees.values()]
          .flat()
          .every((id) => ids.has(id) === present);
      },
      { timeout: 60_000 }
    )
    .toBe(true);
  return latest;
}

function targetBlock(
  tree: BlockTreeResponse,
  fixture: BlockDeleteConformanceCase
): BlockNodePayload {
  const byId = new Map(tree.blocks.map((block) => [block.block_id, block]));
  const markerMatches = fixture.marker
    ? tree.blocks.filter(
        (block) =>
          block.text.includes(fixture.marker ?? '') ||
          JSON.stringify(block.attrs).includes(fixture.marker ?? '')
      )
    : tree.blocks.filter((block) => block.block_type === fixture.blockType);
  if (markerMatches.length !== 1) {
    throw new Error(`expected one marker row for ${fixture.name}, found ${markerMatches.length}`);
  }

  let candidate: BlockNodePayload | undefined = markerMatches[0];
  while (candidate && candidate.block_type !== fixture.blockType) {
    candidate = candidate.parent_block_id ? byId.get(candidate.parent_block_id) : undefined;
  }
  if (!candidate) {
    throw new Error(`could not find ${fixture.blockType} ancestor for ${fixture.name}`);
  }
  return candidate;
}

function subtreeIds(tree: BlockTreeResponse, rootId: string): string[] {
  const children = new Map<string, string[]>();
  for (const block of tree.blocks) {
    if (!block.parent_block_id) continue;
    const siblings = children.get(block.parent_block_id) ?? [];
    siblings.push(block.block_id);
    children.set(block.parent_block_id, siblings);
  }
  const ids: string[] = [];
  const pending = [rootId];
  while (pending.length > 0) {
    const id = pending.pop();
    if (!id) continue;
    ids.push(id);
    pending.push(...(children.get(id) ?? []));
  }
  return ids;
}

function assertTargetSubtrees(
  tree: BlockTreeResponse,
  targets: Map<string, string[]>,
  present: boolean
): void {
  const ids = new Set(tree.blocks.map((block) => block.block_id));
  for (const [name, subtree] of targets) {
    for (const id of subtree) {
      expect(ids.has(id), `${name} subtree block ${id}`).toBe(present);
    }
  }
}

function assertPreservedContent(tree: BlockTreeResponse): void {
  const searchable = tree.blocks
    .map((block) => `${block.text}\n${JSON.stringify(block.attrs)}`)
    .join('\n');
  for (const marker of BLOCK_DELETE_PRESERVED_MARKERS) {
    expect(searchable, `${marker} should survive`).toContain(marker);
  }
}

function assertNoStructuralLeftovers(tree: BlockTreeResponse): void {
  const ids = new Set(tree.blocks.map((block) => block.block_id));
  for (const block of tree.blocks) {
    if (block.parent_block_id) {
      expect(ids.has(block.parent_block_id), `${block.block_id} has a live parent`).toBe(true);
    }
  }
  const emptyListItems = tree.blocks.filter(
    (block) =>
      typeof block.attrs.listStyleType === 'string' &&
      block.text.replaceAll('\u200b', '').trim().length === 0
  );
  expect(emptyListItems, 'no empty list rows remain').toEqual([]);
  const textBlockTypes = new Set([
    'p',
    'h1',
    'h2',
    'h3',
    'h4',
    'h5',
    'h6',
    'blockquote',
    'code_line',
  ]);
  const emptyTopLevelTextBlocks = tree.blocks.filter(
    (block) =>
      block.parent_block_id === null &&
      textBlockTypes.has(block.block_type) &&
      block.text.replaceAll('\u200b', '').trim().length === 0
  );
  expect(emptyTopLevelTextBlocks, 'no empty top-level text blocks remain').toEqual([]);
}

async function expectCanonicalServerRoundTrip(
  request: APIRequestContext,
  library: string,
  path: string,
  markdown: string
): Promise<void> {
  const url = documentApiUrl(library, path);
  await putMarkdown(request, url, markdown);
  expect(await getMarkdown(request, url)).toBe(markdown);
  await getBlocks(request, url);
}

async function putMarkdown(
  request: APIRequestContext,
  documentUrl: string,
  markdown: string
): Promise<void> {
  const response = await request.put(documentUrl, {
    data: markdown,
    headers: { 'content-type': 'text/markdown', 'If-None-Match': '*' },
  });
  await expectOk(response, `put ${documentUrl}`);
}

async function getMarkdown(request: APIRequestContext, documentUrl: string): Promise<string> {
  const response = await request.get(documentUrl);
  await expectOk(response, `get ${documentUrl}`);
  return response.text();
}

async function getBlocks(
  request: APIRequestContext,
  documentUrl: string
): Promise<BlockTreeResponse> {
  const response = await request.get(`${documentUrl}/blocks`);
  await expectOk(response, `get blocks for ${documentUrl}`);
  return (await response.json()) as BlockTreeResponse;
}

async function getReview(
  request: APIRequestContext,
  documentUrl: string
): Promise<ReviewResponse> {
  const response = await request.get(`${documentUrl}/review`);
  await expectOk(response, `get review for ${documentUrl}`);
  return (await response.json()) as ReviewResponse;
}

async function expectOk(
  response: { ok(): boolean; status(): number; text(): Promise<string> },
  label: string
): Promise<void> {
  if (response.ok()) return;
  throw new Error(`${label} failed with ${response.status()}: ${await response.text()}`);
}

async function expectSaved(page: Page): Promise<void> {
  await expect(page.locator('[aria-label="Save status"]')).toContainText('Saved', {
    timeout: 30_000,
  });
}

function suggestionBody(fixture: BlockDeleteConformanceCase): string {
  return `Conformance delete [${fixture.name}]`;
}

function uniqueLibrary(mode: string, workerIndex: number): string {
  return `block-delete-${mode}-${Date.now().toString(36)}-${workerIndex}`;
}

function documentApiUrl(library: string, path: string): string {
  return `${API_ORIGIN}/v1/libraries/${encodeURIComponent(
    library
  )}/documents/${encodeURIComponent(path)}`;
}
