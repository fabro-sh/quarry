import { test, expect } from 'playwright/test';
import { blocks, body, createDocument, openDocument, review, select, selectRange, transaction } from './helpers/native-document';

test('an agent block proposal is editable and its table comment follows acceptance, undo and reload', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Before.\n\nAfter.');
  const [first] = await blocks(request, fixture.url);
  await transaction(request, fixture.url, [{ op: 'suggestion.add_markdown', after_block_id: first.block_id,
    markdown: '## Heading\n\n| Header |\n| --- |\n| TARGET |', body: 'Add detail' }], first.document_clock);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    const proposal = (await review(request, fixture.url)).suggestions[0];
    await expect(body(user.page).locator('h2')).toHaveText('Heading');
    await expect(body(user.page).locator('td')).toHaveText('TARGET');
    const heading = await body(user.page).locator('h2').getAttribute('data-block-id');
    await select(user.page, heading!, 7); await user.page.keyboard.type('!');
    await expect.poll(async () => (await review(request, fixture.url)).suggestions[0].content).toContain('Heading!');
    const id = await body(user.page).locator('td [data-block-id]').getAttribute('data-block-id');
    await select(user.page, id!, 0, 6);
    await user.page.getByRole('button', { name: 'Comment', exact: true }).click();
    await user.page.getByTestId('draft-input').fill('Keep the proposed cell');
    await user.page.getByRole('button', { name: 'Submit comment', exact: true }).click();
    await expect.poll(async () => (await review(request, fixture.url)).comments.length).toBe(1);
    const comment = (await review(request, fixture.url)).comments[0];
    expect(comment.target.attachments[0]).toMatchObject({ owner: { kind: 'proposal', id: proposal.id }, quote: 'TARGET' });
    await expect(body(user.page).locator(`[data-comment-id="${comment.id}"]`)).toHaveText('TARGET');
    expect((await blocks(request, fixture.url)).map((block) => block.text)).toEqual(['Before.', 'After.']);
    await user.page.getByTestId('suggestion-card').getByRole('button', { name: 'Accept suggestion', exact: true }).click();
    await expect.poll(async () => (await blocks(request, fixture.url)).some((block) => block.text === 'TARGET')).toBe(true);
    const accepted = (await review(request, fixture.url)).comments[0].target.attachments[0];
    expect(accepted.owner.kind).toBe('block'); expect(accepted.quote).toBe('TARGET');
    await select(user.page, accepted.owner.id, 0); await user.page.keyboard.press('ControlOrMeta+z');
    await expect.poll(async () => (await review(request, fixture.url)).suggestions[0].status).toBe('open');
    await expect(body(user.page).locator(`[data-comment-id="${comment.id}"]`)).toHaveText('TARGET');
    await user.page.keyboard.press('ControlOrMeta+Shift+z');
    await expect.poll(async () => (await review(request, fixture.url)).suggestions[0].status).toBe('resolved');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload();
    await expect(body(user.page).locator('h2')).toHaveText('Heading!');
    await expect(body(user.page).locator(`[data-comment-id="${comment.id}"]`)).toHaveText('TARGET');
    expect((await review(request, fixture.url)).comments[0].target.attachments[0]).toEqual(accepted);
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('the Plate to-do checkbox in Suggesting mode requires acceptance and preserves its comment', async ({ browser, request }) => {
  const fixture = await createDocument(request, '- [ ] TARGET');
  const [block] = await blocks(request, fixture.url);
  await transaction(request, fixture.url, [{ op: 'comment.add', block_id: block.block_id, start: 0, end: 6, body: 'Keep this task' }], block.document_clock);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await user.page.getByRole('button', { name: 'Document mode', exact: true }).click();
    await user.page.getByRole('menuitem', { name: 'Suggesting', exact: true }).click();
    const checkbox = body(user.page).getByRole('checkbox', { name: 'Toggle to-do', exact: true });
    await checkbox.click();
    await expect.poll(async () => (await review(request, fixture.url)).suggestions.length).toBe(1);
    await expect(checkbox).not.toBeChecked();
    await expect(user.page.getByTestId('suggestion-card')).toContainText('Task → Completed task');
    await user.page.getByTestId('suggestion-card').getByRole('button', { name: 'Accept suggestion', exact: true }).click();
    await expect(checkbox).toBeChecked();
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload();
    await expect(checkbox).toBeChecked();
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('a browser selection ending inside a table cell replaces only the selected characters', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Start TARGET\n\n| Header | Second |\n| --- | --- |\n| Inside TARGET | Keep |\n\nEnd TARGET');
  const initial = await blocks(request, fixture.url);
  const start = initial.find((block) => block.text === 'Start TARGET')!;
  const inside = initial.find((block) => block.text === 'Inside TARGET')!;
  await transaction(request, fixture.url, [{ op: 'comment.add', block_id: inside.block_id, start: 7, end: 13, body: 'Keep this exact target' }], inside.document_clock);
  const user = await openDocument(browser, fixture.path, 'Writer');
  try {
    await selectRange(user.page, { id: start.block_id, offset: 6 }, { id: inside.block_id, offset: 7 });
    await expect(user.page.getByRole('button', { name: 'Comment', exact: true })).toBeVisible();
    await user.page.keyboard.insertText('Replacement');
    await expect.poll(async () => (await blocks(request, fixture.url)).find((block) => block.block_id === start.block_id)?.text).toBe('Start Replacement');
    expect((await blocks(request, fixture.url)).find((block) => block.block_id === inside.block_id)?.text).toBe('TARGET');
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    await expect(body(user.page)).toContainText('Keep');
    await expect(body(user.page)).toContainText('End TARGET');
    await user.page.keyboard.press('ControlOrMeta+z');
    await expect.poll(async () => (await blocks(request, fixture.url)).find((block) => block.block_id === inside.block_id)?.text).toBe('Inside TARGET');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload();
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});
