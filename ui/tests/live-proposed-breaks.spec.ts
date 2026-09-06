import { expect, test } from 'playwright/test';
import { blocks, body, createDocument, openDocument, paste, review, select, transaction } from './helpers/native-document';

test('rich paste stays in its block suggestion and keeps the original comment through undo and acceptance', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Canonical TARGET');
  const [canonical] = await blocks(request, fixture.url);
  await transaction(request, fixture.url, [{ op: 'suggestion.add_markdown', after_block_id: canonical.block_id, markdown: '😀TARGET', body: 'New content' }], canonical.document_clock);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    const proposed = body(user.page).locator('.slate-p[data-block-id^="proposal:"]');
    await expect(proposed).toHaveText(['😀TARGET']);
    const id = (await proposed.first().getAttribute('data-block-id'))!;
    await select(user.page, id, 5, 8);
    await user.page.getByRole('button', { name: 'Comment', exact: true }).click();
    await user.page.getByTestId('draft-input').fill('Keep the original GET');
    await user.page.getByRole('button', { name: 'Submit comment', exact: true }).click();
    await expect.poll(async () => (await review(request, fixture.url)).comments.length).toBe(1);
    await select(user.page, id, 5);
    await paste(user.page, '<p><strong>New</strong></p><p><u>Middle</u></p><p><em>Last</em></p>', 'New\nMiddle\nLast');
    await expect(proposed).toHaveText(['😀TARNew', 'Middle', 'LastGET']);
    await expect(proposed.first().locator('strong')).toHaveText('New');
    await expect(proposed.nth(1).locator('u')).toHaveText('Middle');
    await expect(proposed.last().locator('em')).toHaveText('Last');
    await user.page.keyboard.press('ControlOrMeta+z');
    await expect(proposed).toHaveText(['😀TARGET']);
    await user.page.keyboard.press('ControlOrMeta+Shift+z');
    await expect(proposed).toHaveText(['😀TARNew', 'Middle', 'LastGET']);
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    expect((await blocks(request, fixture.url)).map((block) => block.text)).toEqual(['Canonical TARGET']);
    expect((await review(request, fixture.url)).suggestions).toHaveLength(1);
    await user.page.reload();
    await expect(proposed).toHaveText(['😀TARNew', 'Middle', 'LastGET']);
    await user.page.getByRole('tab', { name: /Comments/ }).click();
    await user.page.getByTestId('suggestion-card').getByRole('button', { name: 'Accept suggestion', exact: true }).click();
    await expect.poll(async () => (await blocks(request, fixture.url)).map((block) => block.text)).toEqual(['Canonical TARGET', '😀TARNew', 'Middle', 'LastGET']);
    await user.page.reload();
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('GET');
    expect((await review(request, fixture.url)).comments[0].target.attachments.map((part: { quote: string }) => part.quote).join('')).toBe('GET');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

for (const code of [false, true]) test(`proposed ${code ? 'code lines' : 'paragraphs'} split, join and retain the caret through an agent edit`, async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Canonical TARGET');
  const [canonical] = await blocks(request, fixture.url);
  await transaction(request, fixture.url, [{ op: 'suggestion.add_markdown', after_block_id: canonical.block_id,
    markdown: code ? '```txt\n😀TARGET\nOther TARGET\n```' : '😀TARGET\n\nOther TARGET', body: 'New content' }], canonical.document_clock);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    const proposed = body(user.page).locator(`${code ? '.slate-code_line' : '.slate-p'}[data-block-id^="proposal:"]`);
    await expect(proposed).toHaveText(['😀TARGET', 'Other TARGET']);
    const first = (await proposed.first().getAttribute('data-block-id'))!;
    await select(user.page, first, 2, 8);
    await user.page.getByRole('button', { name: 'Comment', exact: true }).click();
    await user.page.getByTestId('draft-input').fill('Keep the first occurrence');
    await user.page.getByRole('button', { name: 'Submit comment', exact: true }).click();
    await expect.poll(async () => (await review(request, fixture.url)).comments.length).toBe(1);
    await select(user.page, first, 5); await user.page.keyboard.press('Enter');
    await expect(proposed).toHaveText(['😀TAR', 'GET', 'Other TARGET']);
    expect((await blocks(request, fixture.url)).map((block) => block.text)).toEqual(['Canonical TARGET']);
    const right = (await proposed.nth(1).getAttribute('data-block-id'))!;
    await select(user.page, right, 1);
    expect(await proposed.nth(1).evaluate((element) => {
      const selection = window.getSelection()!;
      if (!element.contains(selection.anchorNode)) return null;
      const range = document.createRange(); range.selectNodeContents(element); range.setEnd(selection.anchorNode!, selection.anchorOffset);
      return range.toString().length;
    })).toBe(1);
    await transaction(request, fixture.url, [{ op: 'replace_block_content', block_id: canonical.block_id, text: 'Agent Canonical TARGET' }], canonical.document_clock);
    await expect(body(user.page)).toContainText('Agent Canonical TARGET');
    await user.page.keyboard.type('!');
    await expect(proposed).toHaveText(['😀TAR', 'G!ET', 'Other TARGET']);
    await user.page.keyboard.press('ControlOrMeta+z');
    await expect(proposed).toHaveText(['😀TAR', 'GET', 'Other TARGET']);
    await select(user.page, right, 0); await user.page.keyboard.press('Backspace');
    await expect(proposed).toHaveText(['😀TARGET', 'Other TARGET']);
    await user.page.keyboard.press('ControlOrMeta+z');
    await expect(proposed).toHaveText(['😀TAR', 'GET', 'Other TARGET']);
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload();
    await expect(proposed).toHaveText(['😀TAR', 'GET', 'Other TARGET']);
    await user.page.getByRole('tab', { name: /Comments/ }).click();
    await user.page.getByTestId('suggestion-card').getByRole('button', { name: 'Accept suggestion', exact: true }).click();
    await expect.poll(async () => (await review(request, fixture.url)).suggestions[0].status).toBe('resolved');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload();
    await expect(body(user.page)).toContainText('Agent Canonical TARGET');
    await expect.poll(async () => (await body(user.page).locator('[data-comment-id]').allTextContents()).join('')).toBe('TARGET');
    await expect.poll(() => body(user.page).locator('[data-comment-id]').evaluateAll((spans) => {
      const owners = new Map<Element, string>();
      for (const span of spans) {
        const block = span.closest('[data-block-id]')!;
        owners.set(block, (owners.get(block) ?? '') + span.textContent);
      }
      return [...owners.values()];
    })).toEqual(['TAR', 'GET']);
    expect((await review(request, fixture.url)).comments[0].target.attachments.map((part: { quote: string }) => part.quote).join('')).toBe('TARGET');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});
