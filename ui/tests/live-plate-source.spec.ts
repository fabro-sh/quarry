import { test, expect } from 'playwright/test';
import { blocks, body, createDocument, openDocument, review, transaction } from './helpers/native-document';

for (const change of ['remove', 'change type'] as const) test(`a private source draft survives an agent ${change} of its block`, async ({ browser, request }) => {
  const fixture = await createDocument(request, '```mermaid\nflowchart LR\nA --> B\n```\n\nArrival marker');
  const initial = await blocks(request, fixture.url), diagram = initial[0], marker = initial[1];
  const user = await openDocument(browser, fixture.path, 'Writer');
  try {
    await user.page.getByRole('button', { name: 'Edit Mermaid source', exact: true }).click();
    await user.page.getByRole('textbox', { name: 'Mermaid source', exact: true }).fill('flowchart LR\nPRIVATE --> DRAFT');
    await transaction(request, fixture.url, [
      change === 'remove' ? { op: 'delete_block', block_id: diagram.block_id } : { op: 'set_block_type', block_id: diagram.block_id, block_type: 'p', attrs: {} },
      { op: 'replace_block_content', block_id: marker.block_id, text: 'Agent change received' },
    ], diagram.document_clock);
    await expect(body(user.page)).toContainText('Agent change received');
    const recovery = user.page.getByRole('region', { name: 'Unsaved source drafts', exact: true });
    await expect(recovery).toBeVisible();
    await expect(recovery.getByRole('textbox', { name: 'Mermaid source', exact: true })).toHaveValue('flowchart LR\nPRIVATE --> DRAFT');
    expect((await blocks(request, fixture.url)).some((block) => block.block_type === 'mermaid')).toBe(false);
    await recovery.getByRole('button', { name: 'Discard draft', exact: true }).click();
    await expect(recovery).toHaveCount(0);
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('source drafts survive remote edits and Suggesting mode requires acceptance before changing a diagram', async ({ browser, request }) => {
  test.setTimeout(90000);
  const fixture = await createDocument(request, '```mermaid\nflowchart LR\nA --> B\n```\n\n<custom>Original source</custom>\n\nArrival marker\n');
  const initial = await blocks(request, fixture.url);
  const diagram = initial.find((block) => block.block_type === 'mermaid')!;
  const raw = initial.find((block) => block.block_type === 'raw_markdown')!;
  const marker = initial.find((block) => block.text === 'Arrival marker')!;
  expect(diagram).toBeDefined(); expect(raw).toBeDefined(); expect(marker).toBeDefined();
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  const attrs = async (id: string) => (await (await request.get(`${fixture.url}/blocks`)).json()).blocks.find((block: { block_id: string }) => block.block_id === id).attrs;
  try {
    await user.page.getByRole('button', { name: 'Document mode', exact: true }).click();
    await user.page.getByRole('menuitem', { name: 'Suggesting', exact: true }).click();
    await user.page.getByRole('button', { name: 'Edit Mermaid source', exact: true }).click();
    await user.page.getByRole('textbox', { name: 'Mermaid source', exact: true }).fill('flowchart LR\nA --> C');
    expect((await attrs(diagram.block_id)).code).toContain('B');
    await user.page.getByRole('button', { name: 'Save source', exact: true }).click();
    await expect.poll(async () => (await review(request, fixture.url)).suggestions.length).toBe(1);
    expect((await attrs(diagram.block_id)).code).toContain('B');
    const suggestion = user.page.getByLabel('Suggestion by Reviewer', { exact: true });
    await expect(suggestion.getByLabel('Proposed source', { exact: true })).toHaveText('flowchart LR\nA --> C');
    await suggestion.getByRole('button', { name: 'Accept suggestion', exact: true }).click();
    await expect.poll(async () => (await attrs(diagram.block_id)).code).toBe('flowchart LR\nA --> C');
    await expect(body(user.page).getByTestId('mermaid-diagram').locator('svg')).toContainText('C');

    await user.page.getByRole('button', { name: 'Edit source', exact: true }).click();
    const draft = user.page.getByRole('textbox', { name: 'Markdown source', exact: true });
    await draft.fill('Private draft');
    const currentRaw = (await blocks(request, fixture.url)).find((block) => block.block_id === raw.block_id)!;
    await transaction(request, fixture.url, [
      { op: 'set_block_attrs', block_id: currentRaw.block_id, attrs: { markdown: '<custom>Agent source</custom>' } },
      { op: 'replace_block_content', block_id: marker.block_id, text: 'Remote source received' },
    ], currentRaw.document_clock);
    await expect(body(user.page)).toContainText('Remote source received');
    await expect(draft).toHaveValue('Private draft');
    await user.page.getByRole('button', { name: 'Save source', exact: true }).click();
    await expect(user.page.getByRole('alert')).toContainText('source changed');
    await expect(draft).toHaveValue('Private draft');
    expect((await attrs(raw.block_id)).markdown).toBe('<custom>Agent source</custom>');
    await user.page.getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(body(user.page).getByTestId('raw-markdown-block')).toHaveText('<custom>Agent source</custom>');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload();
    await expect(body(user.page).getByTestId('mermaid-diagram').locator('svg')).toContainText('C');
    await expect(body(user.page).getByTestId('raw-markdown-block')).toHaveText('<custom>Agent source</custom>');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('a proposed diagram source is editable and its private draft follows agent acceptance', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Before.\n\nAfter.');
  const initial = await blocks(request, fixture.url);
  await transaction(request, fixture.url, [{ op: 'suggestion.add_markdown', after_block_id: initial[0].block_id,
    markdown: '```mermaid\nflowchart LR\nA --> B\n```', body: 'Add a diagram' }], initial[0].document_clock);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    const proposal = (await review(request, fixture.url)).suggestions[0];
    await user.page.getByRole('button', { name: 'Edit Mermaid source', exact: true }).click();
    const source = user.page.getByRole('textbox', { name: 'Mermaid source', exact: true });
    await source.fill('flowchart LR\nSaved --> Source');
    await user.page.getByRole('button', { name: 'Save source', exact: true }).click();
    await expect.poll(async () => (await review(request, fixture.url)).suggestions[0].content).toContain('Saved --> Source');
    expect((await blocks(request, fixture.url)).map((block) => block.text)).toEqual(initial.map((block) => block.text));
    await user.page.getByRole('button', { name: 'Edit Mermaid source', exact: true }).click();
    await source.fill('flowchart LR\nPrivate --> Draft');
    const currentReview = await review(request, fixture.url);
    await transaction(request, fixture.url, [{ op: 'suggestion.accept', item_id: proposal.id },
      { op: 'replace_block_content', block_id: initial[1].block_id, text: 'Acceptance received' }], currentReview.baseToken);
    await expect(body(user.page)).toContainText('Acceptance received');
    await expect.poll(async () => (await blocks(request, fixture.url)).some((block) => block.block_type === 'mermaid')).toBe(true);
    await expect(user.page.getByTestId('suggestion-card')).toHaveCount(0);
    await expect(source).toHaveValue('flowchart LR\nPrivate --> Draft');
    await expect(user.page.getByRole('region', { name: 'Unsaved source drafts', exact: true })).toHaveCount(0);
    await user.page.getByRole('button', { name: 'Save source', exact: true }).click();
    await expect(body(user.page).getByTestId('mermaid-diagram').locator('svg')).toContainText('Private');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload();
    await expect(body(user.page).getByTestId('mermaid-diagram').locator('svg')).toContainText('Private');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});
