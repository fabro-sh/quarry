import { expect, test } from 'playwright/test';
import { blocks, body, createDocument, openDocument, review, selectRange, transaction } from './helpers/native-document';

test('a replacement suggestion crosses a table and preserves unselected commented text after acceptance', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Before TARGET\n\n| Head |\n| --- |\n| Before TARGET |');
  const initial = await blocks(request, fixture.url), first = initial[0], cell = initial.at(-1)!;
  await transaction(request, fixture.url, [{ op: 'comment.add', block_id: cell.block_id, start: 7, end: 13, body: 'Keep these characters' }], cell.document_clock);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await user.page.getByRole('button', { name: 'Document mode', exact: true }).click();
    await user.page.getByRole('menuitem', { name: 'Suggesting', exact: true }).click();
    await selectRange(user.page, { id: first.block_id, offset: 7 }, { id: cell.block_id, offset: 7 });
    await user.page.keyboard.type('NEW');
    await expect.poll(async () => (await review(request, fixture.url)).suggestions.map((suggestion) => suggestion.content)).toEqual(['NEW']);
    expect((await blocks(request, fixture.url)).map((block) => block.text)).toEqual(initial.map((block) => block.text));
    await user.page.getByTestId('suggestion-card').getByRole('button', { name: 'Accept suggestion', exact: true }).click();
    await expect.poll(async () => (await blocks(request, fixture.url))[0].text).toBe('Before NEW');
    expect((await blocks(request, fixture.url)).at(-1)?.text).toBe('TARGET');
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload();
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('typing a suggestion into an empty document saves only the proposal until acceptance', async ({ browser, request }) => {
  const fixture = await createDocument(request, '');
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await user.page.getByRole('button', { name: 'Document mode', exact: true }).click();
    await user.page.getByRole('menuitem', { name: 'Suggesting', exact: true }).click();
    await body(user.page).locator('.slate-p').click(); await user.page.keyboard.type('New paragraph');
    await expect.poll(async () => (await review(request, fixture.url)).suggestions.map((suggestion) => suggestion.content)).toEqual(['New paragraph\n']);
    expect(await blocks(request, fixture.url)).toEqual([]);
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload();
    await expect(body(user.page)).toContainText('New paragraph');
    await user.page.getByRole('tab', { name: /Comments/ }).click();
    await user.page.getByTestId('suggestion-card').getByRole('button', { name: 'Accept suggestion', exact: true }).click();
    await expect.poll(async () => (await blocks(request, fixture.url)).map((block) => block.text)).toEqual(['New paragraph']);
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload(); await expect(body(user.page)).toHaveText('New paragraph');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('the Table control creates a suggestion that can be typed in, reviewed and accepted', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Before TARGET.\n\nAfter.');
  const initial = await blocks(request, fixture.url);
  await transaction(request, fixture.url, [{ op: 'comment.add', block_id: initial[0].block_id, start: 7, end: 13, body: 'Original target' }], initial[0].document_clock);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await user.page.getByRole('button', { name: 'Document mode', exact: true }).click();
    await user.page.getByRole('menuitem', { name: 'Suggesting', exact: true }).click();
    await selectRange(user.page, { id: initial[0].block_id, offset: 0 }, { id: initial[0].block_id, offset: 6 });
    await user.page.getByRole('button', { name: 'Turn into', exact: true }).click();
    await user.page.getByRole('menuitem', { name: 'Table', exact: true }).click();
    await expect(body(user.page).locator('table')).toHaveCount(1);
    await expect.poll(async () => (await review(request, fixture.url)).suggestions.length).toBe(1);
    expect((await blocks(request, fixture.url)).map((block) => block.text)).toEqual(initial.map((block) => block.text));
    await body(user.page).locator('th .slate-p').first().click(); await user.page.keyboard.type('Proposed header');
    await expect(body(user.page).locator('th').first()).toContainText('Proposed header');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload(); await expect(body(user.page).locator('th').first()).toContainText('Proposed header');
    await user.page.getByRole('tab', { name: /Comments/ }).click();
    await user.page.getByTestId('suggestion-card').getByRole('button', { name: 'Accept suggestion', exact: true }).click();
    await expect.poll(async () => (await blocks(request, fixture.url)).some((block) => block.text === 'Proposed header')).toBe(true);
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload(); await expect(body(user.page).locator('th').first()).toContainText('Proposed header');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});
