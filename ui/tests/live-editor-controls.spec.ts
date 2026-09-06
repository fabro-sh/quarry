import { expect, test, type Page } from 'playwright/test';
import { blocks, body, createDocument, openDocument, select, transaction } from './helpers/native-document';

function block(page: Page, id: string) { return body(page).locator(`[data-block-id="${id}"]`); }
async function actions(page: Page, id: string) {
  const element = block(page, id); await element.scrollIntoViewIfNeeded(); await element.hover();
  await element.locator('..').getByRole('button', { name: /^(Drag to move block|Block actions)$/ }).click();
  return page.getByRole('menu', { name: 'Block actions', exact: true });
}

test('review editing, target focus, contents and block conversion retain exact native targets', async ({ browser, request }) => {
  const fixture = await createDocument(request, '# First\n\nBefore TARGET after.\n\n' + 'Filler paragraph.\n\n'.repeat(40) + '# Last\n\nEnd.');
  const initial = await blocks(request, fixture.url); const paragraph = initial[1];
  await transaction(request, fixture.url, [{ op: 'comment.add', block_id: paragraph.block_id, start: 7, end: 13, body: 'Original review' }], paragraph.document_clock);
  const user = await openDocument(browser, fixture.path, 'Writer');
  try {
    await user.page.getByRole('tab', { name: /Comments/ }).click();
    const thread = user.page.getByTestId('comment-card');
    await thread.hover(); await expect(body(user.page).locator('[data-comment-id]')).toHaveClass(/ring-1/);
    await thread.locator('..').getByRole('button', { name: 'Show target', exact: true }).click();
    await expect.poll(() => user.page.evaluate(() => getSelection()?.toString())).toBe('TARGET');
    const edit = async () => {
      await thread.getByRole('button', { name: 'Comment actions', exact: true }).click();
      await user.page.getByRole('menuitem', { name: 'Edit', exact: true }).click();
      await thread.getByRole('textbox', { name: 'Edit comment', exact: true }).fill('Edited review');
    };
    await edit(); await thread.getByRole('button', { name: 'Cancel edit', exact: true }).click();
    await expect(thread.getByText('Original review', { exact: true })).toBeVisible();
    await edit(); await thread.getByRole('button', { name: 'Save edit', exact: true }).click();
    await expect(thread.getByText('Edited review', { exact: true })).toBeVisible();
    await select(user.page, initial[0].block_id, 0); await user.page.keyboard.press('ArrowRight');
    const contents = user.page.getByRole('navigation', { name: 'Table of contents' });
    await contents.locator('..').hover();
    await expect(contents).toHaveCSS('opacity', '1');
    await contents.getByRole('button', { name: 'Last', exact: true }).click();
    const last = initial.find((entry) => entry.text === 'Last')!;
    await expect(block(user.page, last.block_id)).toBeInViewport();
    await select(user.page, last.block_id, 0); await user.page.keyboard.type('Updated ');
    await expect(body(user.page).locator('h1').last()).toHaveText('Updated Last');
    await block(user.page, paragraph.block_id).scrollIntoViewIfNeeded();
    await select(user.page, paragraph.block_id, 0, paragraph.text.length);
    await user.page.getByRole('button', { name: 'Turn into', exact: true }).click();
    await user.page.getByRole('menuitem', { name: 'Code', exact: true }).click();
    await expect(body(user.page).locator('.slate-code_line')).toHaveText('Before TARGET after.');
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    const code = (await blocks(request, fixture.url)).find((entry) => entry.block_type === 'code_block')!;
    await (await actions(user.page, code.block_id)).getByRole('menuitem', { name: 'Duplicate', exact: true }).click();
    await expect(body(user.page).locator('.slate-code_block')).toHaveCount(2);
    await (await actions(user.page, code.block_id)).getByRole('menuitem', { name: 'Delete', exact: true }).click();
    await expect(body(user.page).locator('.slate-code_block')).toHaveCount(1);
    await expect(thread.locator('..')).toContainText('The target block was removed.');
    await body(user.page).focus(); await user.page.keyboard.press('ControlOrMeta+z');
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload(); await expect(thread.getByText('Edited review', { exact: true })).toBeVisible();
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('dragging the selected block handle changes order without moving its comment to repeated text', async ({ browser, browserName, request }) => {
  test.skip(browserName === 'webkit', 'Safari does not support native HTML5 block dragging inside contentEditable');
  const fixture = await createDocument(request, 'First TARGET\n\nSecond TARGET\n\nLast.');
  const initial = await blocks(request, fixture.url);
  await transaction(request, fixture.url, [{ op: 'comment.add', block_id: initial[0].block_id, start: 6, end: 12, body: 'First only' }], initial[0].document_clock);
  const user = await openDocument(browser, fixture.path, 'Writer');
  try {
    const first = block(user.page, initial[0].block_id), last = block(user.page, initial[2].block_id);
    const handle = first.locator('..').getByRole('button', { name: 'Drag to move block' });
    await select(user.page, initial[0].block_id, 0);
    const drag = async (bottom = false) => {
      await first.hover(); const from = (await handle.boundingBox())!, to = (await last.boundingBox())!;
      const y = bottom ? to.y + to.height - 2 : to.y + 2;
      await user.page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
      await user.page.mouse.down(); await user.page.mouse.move(from.x + from.width + 10, from.y + from.height / 2, { steps: 3 });
      await user.page.mouse.move(to.x + 100, y, { steps: 10 }); await user.page.mouse.move(to.x + 101, y); await user.page.mouse.up();
    };
    await drag();
    await expect.poll(async () => (await blocks(request, fixture.url)).map((entry) => entry.block_id)).toEqual([initial[1].block_id, initial[0].block_id, initial[2].block_id]);
    await expect(first.locator('[data-comment-id]')).toHaveText('TARGET');
    await expect(block(user.page, initial[1].block_id).locator('[data-comment-id]')).toHaveCount(0);
    await first.hover(); const from = (await handle.boundingBox())!, to = (await last.boundingBox())!;
    await user.page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
    await user.page.mouse.down(); await user.page.mouse.move(to.x + 100, to.y + to.height - 2, { steps: 5 });
    await user.page.keyboard.press('Escape'); await user.page.mouse.up();
    expect((await blocks(request, fixture.url)).map((entry) => entry.block_id)).toEqual([initial[1].block_id, initial[0].block_id, initial[2].block_id]);
    await drag(true);
    await expect.poll(async () => (await blocks(request, fixture.url)).map((entry) => entry.block_id)).toEqual([initial[1].block_id, initial[2].block_id, initial[0].block_id]);
    await user.page.reload(); await expect(first.locator('[data-comment-id]')).toHaveText('TARGET');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('a drag in Suggesting mode keeps canonical order until acceptance and retains a concurrent agent edit', async ({ browser, browserName, request }) => {
  test.skip(browserName === 'webkit', 'Safari does not support native HTML5 block dragging inside contentEditable');
  const fixture = await createDocument(request, 'First TARGET\n\nSecond TARGET\n\nLast.');
  const initial = await blocks(request, fixture.url);
  await transaction(request, fixture.url, [{ op: 'comment.add', block_id: initial[0].block_id, start: 6, end: 12, body: 'First only' }], initial[0].document_clock);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await user.page.getByRole('button', { name: 'Document mode', exact: true }).click();
    await user.page.getByRole('menuitem', { name: 'Suggesting', exact: true }).click();
    const first = block(user.page, initial[0].block_id), last = block(user.page, initial[2].block_id);
    const handle = first.locator('..').getByRole('button', { name: 'Drag to move block' });
    await select(user.page, initial[0].block_id, 0); await first.hover();
    const from = (await handle.boundingBox())!, to = (await last.boundingBox())!;
    await user.page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
    await user.page.mouse.down(); await user.page.mouse.move(from.x + from.width + 10, from.y + from.height / 2, { steps: 3 });
    await user.page.mouse.move(to.x + 100, to.y + to.height - 2, { steps: 10 });
    await user.page.mouse.move(to.x + 101, to.y + to.height - 2); await user.page.mouse.up();
    const card = user.page.getByTestId('suggestion-card');
    await expect(card).toContainText('Move: First TARGET → end of document');
    expect((await blocks(request, fixture.url)).map((entry) => entry.block_id)).toEqual(initial.map((entry) => entry.block_id));
    await transaction(request, fixture.url, [{ op: 'replace_block_content', block_id: initial[0].block_id, text: 'Agent First TARGET' }], initial[0].document_clock);
    await expect(first).toContainText('Agent First TARGET');
    await card.getByRole('button', { name: 'Accept suggestion', exact: true }).click();
    await expect.poll(async () => (await blocks(request, fixture.url)).map((entry) => entry.block_id)).toEqual([initial[1].block_id, initial[2].block_id, initial[0].block_id]);
    await expect(first.locator('[data-comment-id]')).toHaveText('TARGET');
    await expect(block(user.page, initial[1].block_id).locator('[data-comment-id]')).toHaveCount(0);
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload(); await expect(first).toContainText('Agent First TARGET');
    await expect(first.locator('[data-comment-id]')).toHaveText('TARGET');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});
