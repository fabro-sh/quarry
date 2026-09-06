import { expect, test } from 'playwright/test';
import { blocks, body, createDocument, openDocument, review, select, transaction } from './helpers/native-document';

test('the restored Plate toolbar formats commented text and saves real typing through Automerge', async ({ browser, request }) => {
  const fixture = await createDocument(request, '# Plate editor\n\nBefore TARGET after.\n\nSecond TARGET.');
  const original = await blocks(request, fixture.url);
  const target = original.find((block) => block.text === 'Before TARGET after.')!;
  await transaction(request, fixture.url, [{ op: 'comment.add', block_id: target.block_id, start: 7, end: 13, body: 'Keep this exact target' }], target.document_clock);
  const user = await openDocument(browser, fixture.path, 'Writer');
  try {
    await expect(user.page.getByLabel('Plate markdown editor')).toBeVisible();
    await select(user.page, target.block_id, 0);
    await user.page.keyboard.type('Fast ');
    await expect.poll(async () => (await blocks(request, fixture.url)).find((block) => block.block_id === target.block_id)?.text).toBe('Fast Before TARGET after.');
    await select(user.page, target.block_id, 12, 18);
    await user.page.getByRole('button', { name: 'Bold', exact: true }).click();
    await expect(body(user.page).locator('strong')).toHaveText('TARGET');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload();
    await expect(body(user.page).locator('strong')).toHaveText('TARGET');
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    expect((await review(request, fixture.url)).comments.some((item: { body?: string }) => item.body === 'Keep this exact target')).toBe(true);
    await user.page.screenshot({ path: test.info().outputPath('restored-plate-first-browser.png'), fullPage: true });
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});


test('a formatting suggestion uses the restored Bold control and keeps comments through acceptance and undo', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Before TARGET after.');
  const [target] = await blocks(request, fixture.url);
  await transaction(request, fixture.url, [{ op: 'comment.add', block_id: target.block_id, start: 7, end: 13, body: 'Keep these characters' }], target.document_clock);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await user.page.getByRole('button', { name: 'Document mode' }).click();
    await user.page.getByRole('menuitem', { name: 'Suggesting' }).click();
    await select(user.page, target.block_id, 7, 13);
    await user.page.getByLabel('Formatting', { exact: true }).getByRole('button', { name: 'Bold', exact: true }).click();
    await expect.poll(async () => (await review(request, fixture.url)).suggestions.length).toBe(1);
    const suggestion = (await review(request, fixture.url)).suggestions[0];
    expect(suggestion.kind).toBe('format');
    expect(suggestion.action.name).toBe('bold');
    await expect(body(user.page).locator('strong')).toHaveCount(0);
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    await select(user.page, target.block_id, 7, 13);
    await user.page.getByLabel('Formatting', { exact: true }).getByRole('button', { name: 'Accept suggestion' }).click();
    await expect(body(user.page).locator('strong')).toHaveText('TARGET');
    await expect.poll(async () => (await review(request, fixture.url)).suggestions[0].status).toBe('resolved');
    await user.page.keyboard.press('ControlOrMeta+z');
    await expect(body(user.page).locator('strong')).toHaveCount(0);
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload();
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    await expect(body(user.page).locator('strong')).toHaveCount(0);
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('a private Plate comment draft follows an agent prefix and a second browser split', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Before TARGET after.\n\nDuplicate TARGET elsewhere.');
  const [target, duplicate] = await blocks(request, fixture.url);
  const writer = await openDocument(browser, fixture.path, 'Writer');
  const reviewer = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await select(reviewer.page, target.block_id, 7, 13);
    await reviewer.page.getByRole('button', { name: 'Comment', exact: true }).click();
    await reviewer.page.getByTestId('draft-input').fill('Keep the exact target');
    expect((await review(request, fixture.url)).comments).toHaveLength(0);
    await expect(writer.page.getByTestId('draft-composer')).toHaveCount(0);
    await transaction(request, fixture.url, [{ op: 'replace_block_content', block_id: target.block_id, text: 'Agent Before TARGET after.' }], target.document_clock);
    await expect(body(writer.page)).toContainText('Agent Before TARGET');
    await select(writer.page, target.block_id, 16);
    await writer.page.keyboard.press('Enter');
    await expect.poll(async () => (await blocks(request, fixture.url)).length).toBe(3);
    await reviewer.page.getByRole('button', { name: 'Submit comment', exact: true }).click();
    await expect.poll(async () => (await review(request, fixture.url)).comments.length).toBe(1);
    const comment = (await review(request, fixture.url)).comments[0];
    expect(comment.target.attachments.map((part: { quote: string }) => part.quote).join('')).toBe('TARGET');
    expect(comment.target.attachments.every((part: { owner: { id: string } }) => part.owner.id !== duplicate.block_id)).toBe(true);
    await expect.poll(() => body(reviewer.page).locator('[data-comment-id]').allTextContents()).toEqual(['TAR', 'GET']);
    await expect(reviewer.page.locator('.quarry-remote-caret')).toContainText('Writer');
    await expect(writer.page.locator('.quarry-remote-caret')).toContainText('Reviewer');
    expect(writer.errors).toEqual([]); expect(reviewer.errors).toEqual([]);
  } finally { await writer.context.close(); await reviewer.context.close(); }
});


test('real suggesting-mode typing stays in one inline proposal and its comment follows acceptance', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Before TARGET after.');
  const [target] = await blocks(request, fixture.url);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await user.page.getByRole('button', { name: 'Document mode' }).click();
    await user.page.getByRole('menuitem', { name: 'Suggesting' }).click();
    await select(user.page, target.block_id, 7, 13);
    await user.page.keyboard.type('APPROVED');
    await expect.poll(async () => (await review(request, fixture.url)).suggestions.map((item: { content: string }) => item.content)).toEqual(['APPROVED']);
    expect((await blocks(request, fixture.url))[0].text).toBe('Before TARGET after.');
    const insertion = body(user.page).locator('.slate-quarry_proposal');
    await expect(insertion).toHaveText('APPROVED');
    await insertion.evaluate((element) => {
      const editor = element.closest('[contenteditable]') as HTMLElement; editor.focus();
      const range = document.createRange(); range.selectNodeContents(element);
      const selection = window.getSelection()!; selection.removeAllRanges(); selection.addRange(range);
      document.dispatchEvent(new Event('selectionchange'));
    });
    await user.page.getByRole('button', { name: 'Comment', exact: true }).click();
    await user.page.getByTestId('draft-input').fill('Keep these proposed characters');
    await user.page.getByRole('button', { name: 'Submit comment', exact: true }).click();
    await expect.poll(async () => (await review(request, fixture.url)).comments.length).toBe(1);
    const card = user.page.getByTestId('suggestion-card');
    await card.getByRole('button', { name: 'Accept suggestion' }).click();
    await expect.poll(async () => (await blocks(request, fixture.url))[0].text).toBe('Before APPROVED after.');
    const comment = (await review(request, fixture.url)).comments[0];
    expect(comment.target.attachments[0]).toMatchObject({ owner: { kind: 'block', id: target.block_id }, quote: 'APPROVED' });
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('APPROVED');
    await user.page.reload();
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('APPROVED');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('closing the mode menu preserves a new editor selection and Escape still returns focus to the button', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'TARGET');
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    const mode = user.page.getByRole('button', { name: 'Document mode' });
    await mode.click();
    // Force the ordering that occurs when the user returns to the document
    // before Radix's deferred focus restoration runs.
    await user.page.getByRole('menu').evaluate((menu) => {
      menu.addEventListener('focusScope.autoFocusOnUnmount', () => {
        const editor = document.querySelector('[aria-label="Plate markdown editor"]') as HTMLElement;
        editor.focus();
        const text = editor.querySelector('[data-slate-string]')!.firstChild!;
        window.getSelection()!.setBaseAndExtent(text, 0, text, 6);
        document.dispatchEvent(new Event('selectionchange'));
      }, { once: true });
    });
    await user.page.getByRole('menuitem', { name: 'Suggesting' }).click();
    await expect(body(user.page)).toBeFocused();
    await user.page.keyboard.type('APPROVED');
    await expect.poll(async () => (await review(request, fixture.url)).suggestions.map((item: { content: string }) => item.content)).toEqual(['APPROVED']);
    expect((await blocks(request, fixture.url))[0].text).toBe('TARGET');
    await mode.click();
    await user.page.keyboard.press('Escape');
    await expect(mode).toBeFocused();
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});
