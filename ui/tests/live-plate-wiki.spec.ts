import { expect, test } from 'playwright/test';
import { blocks, body, createDocument, openDocument, review, select, transaction } from './helpers/native-document';

test('partial wiki formatting preserves the browser chip, exported link and native review through reload', async ({ browser, request }) => {
  const syntax = '[[Other#Part|😀label]]', markdown = `See *${syntax}* here. ${syntax}\n`;
  const fixture = await createDocument(request, markdown);
  const first = (await blocks(request, fixture.url))[0];
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    const chips = body(user.page).getByTestId('wikilink');
    await expect(chips).toHaveCount(2);
    await expect(chips.first().locator('em')).toHaveText('😀label');
    const start = 4 + syntax.indexOf('😀');
    await transaction(request, fixture.url, [
      { op: 'comment.add', block_id: first.block_id, start, end: start + 2, body: 'Keep this exact emoji' },
      { op: 'add_mark', block_id: first.block_id, start, end: start + 2, marks: { bold: true } },
    ], first.document_clock);
    await expect(chips.first().locator('strong')).toHaveText('😀');
    expect(await (await request.get(fixture.url)).text()).toBe(markdown);
    await user.page.reload();
    await expect(chips).toHaveCount(2);
    await expect(chips.first().locator('strong')).toHaveText('😀');
    await expect(chips.last().locator('strong')).toHaveCount(0);
    await expect(chips.first()).toHaveAttribute('data-comment-id');
    await expect(chips.last()).not.toHaveAttribute('data-comment-id');
    expect((await review(request, fixture.url)).comments[0].target.attachments.map((part: { quote: string }) => part.quote).join('')).toBe('😀');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('wiki chip review targets the complete syntax through formatting, concurrent prefix and reload', async ({ browser, request }) => {
  const syntax = '[[Other#Part|label]]';
  const fixture = await createDocument(request, `See ${syntax} here. ${syntax}`);
  const first = (await blocks(request, fixture.url))[0];
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    const chips = body(user.page).getByTestId('wikilink');
    await expect(chips).toHaveCount(2);
    const selectChip = async () => user.page.evaluate(() => {
      const chip = document.querySelector('[data-testid="wikilink"]')!.closest('[data-slate-node="element"]')!;
      const before = chip.previousElementSibling!.querySelector('[data-slate-string]')!.firstChild!;
      const after = chip.nextElementSibling!.querySelector('[data-slate-string]')!.firstChild!;
      (chip.closest('[contenteditable="true"]') as HTMLElement).focus();
      getSelection()!.setBaseAndExtent(before, before.textContent!.length, after, 0);
      document.dispatchEvent(new Event('selectionchange'));
    });
    await selectChip();
    await user.page.getByRole('button', { name: 'Comment', exact: true }).click();
    await user.page.getByTestId('draft-input').fill('Keep this wiki target');
    await expect(chips.first()).toHaveAttribute('data-comment-draft', 'true');
    await user.page.getByRole('button', { name: 'Submit comment', exact: true }).click();
    await expect.poll(async () => (await review(request, fixture.url)).comments.length).toBe(1);
    const target = (await review(request, fixture.url)).comments[0].target;
    expect(target.attachments.map((part: { quote: string }) => part.quote).join('')).toBe(syntax);
    await expect(chips.first()).toHaveAttribute('data-comment-id');
    await expect(chips.last()).not.toHaveAttribute('data-comment-id');
    await selectChip(); await user.page.keyboard.press('ControlOrMeta+b');
    await expect(chips.first().locator('strong')).toHaveText('label');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await transaction(request, fixture.url, [{ op: 'replace_block_content', block_id: first.block_id, text: `Agent See ${syntax} here. ${syntax}` }], first.document_clock);
    await expect(body(user.page)).toContainText('Agent See');
    await select(user.page, first.block_id, 0); await user.page.keyboard.type('New ');
    await expect.poll(async () => (await blocks(request, fixture.url))[0].text).toBe(`New Agent See ${syntax} here. ${syntax}`);
    await user.page.reload();
    await expect(chips).toHaveCount(2);
    await expect(chips.first()).toHaveAttribute('data-comment-id');
    expect((await review(request, fixture.url)).comments[0].target.attachments.map((part: { quote: string }) => part.quote).join('')).toBe(syntax);
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});
