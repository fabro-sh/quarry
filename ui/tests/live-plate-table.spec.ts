import { expect, test } from 'playwright/test';
import { blocks, body, createDocument, openDocument, review, select, transaction } from './helpers/native-document';

test('column insertion and deletion retain alignment and comments on the original cell', async ({ browser, request }) => {
  const fixture = await createDocument(request, '| Left | Right |\n| --- | --- |\n| Other | TARGET |');
  const initial = await blocks(request, fixture.url), target = initial.find((block) => block.text === 'TARGET')!;
  await transaction(request, fixture.url, [{ op: 'comment.add', block_id: target.block_id, start: 0, end: 6, body: 'Keep target' }], target.document_clock);
  const user = await openDocument(browser, fixture.path, 'Writer');
  try {
    const menu = async (column: number, action: string) => {
      await body(user.page).locator('th').nth(column).getByRole('button', { name: 'Column options' }).click();
      await user.page.getByRole('menuitem', { name: action, exact: true }).click();
    };
    await menu(1, 'Align right');
    await menu(1, 'Insert column left');
    await expect(body(user.page).locator('th')).toHaveCount(3);
    await expect(body(user.page).locator('th').last()).toHaveCSS('text-align', 'right');
    await expect(body(user.page).locator('td').last()).toHaveCSS('text-align', 'right');
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    await menu(1, 'Delete column');
    await expect(body(user.page).locator('th')).toHaveCount(2);
    await expect(body(user.page).locator('th').last()).toHaveCSS('text-align', 'right');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    expect((await blocks(request, fixture.url)).find((block) => block.text === 'TARGET')?.block_id).toBe(target.block_id);
    await user.page.reload();
    await expect(body(user.page).locator('td').last()).toHaveCSS('text-align', 'right');
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('proposed table controls preserve original cell comments through edits, reload and acceptance', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Before TARGET.\n\nAfter.');
  const initial = await blocks(request, fixture.url);
  await transaction(request, fixture.url, [{ op: 'suggestion.add_markdown', after_block_id: initial[0].block_id,
    markdown: '| Left | Right |\n| --- | --- |\n| Other | TARGET |', body: 'Add a table' }], initial[0].document_clock);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await expect(body(user.page).locator('table')).toHaveCount(1);
    const id = await body(user.page).locator('td .slate-p').last().getAttribute('data-block-id');
    await select(user.page, id!, 0, 6);
    await user.page.getByRole('button', { name: 'Comment', exact: true }).click();
    await user.page.getByTestId('draft-input').fill('Keep the proposed cell');
    await user.page.getByRole('button', { name: 'Submit comment', exact: true }).click();
    await expect.poll(async () => (await review(request, fixture.url)).comments.length).toBe(1);
    await expect(user.page.getByLabel('Formatting', { exact: true })).toHaveCount(0);
    const original = (await review(request, fixture.url)).comments[0];
    const menu = async (column: number, action: string) => {
      await body(user.page).locator('th').nth(column).getByRole('button', { name: 'Column options' }).click();
      await user.page.getByRole('menuitem', { name: action, exact: true }).click();
    };
    await menu(1, 'Align right'); await menu(1, 'Insert column left');
    await expect(body(user.page).locator('th')).toHaveCount(3);
    await body(user.page).locator('td .slate-p').nth(1).click(); await user.page.keyboard.type('New cell');
    await expect(body(user.page).locator('td').nth(1)).toContainText('New cell');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    expect((await blocks(request, fixture.url)).map((b) => b.text)).toEqual(initial.map((b) => b.text));
    await user.page.reload();
    await expect(body(user.page).locator('td').nth(1)).toContainText('New cell');
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    await menu(1, 'Delete column'); await expect(body(user.page).locator('th')).toHaveCount(2);
    await user.page.getByRole('tab', { name: /Comments/ }).click();
    await user.page.getByTestId('suggestion-card').getByRole('button', { name: 'Accept suggestion', exact: true }).click();
    await expect.poll(async () => (await blocks(request, fixture.url)).some((b) => b.text === 'TARGET')).toBe(true);
    const comment = (await review(request, fixture.url)).comments.find((c) => c.id === original.id);
    expect(comment.target.attachments[0]).toMatchObject({ owner: { kind: 'block' }, quote: 'TARGET' });
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload();
    await expect(body(user.page).locator('td').last()).toHaveCSS('text-align', 'right');
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});
