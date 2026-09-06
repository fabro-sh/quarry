import { expect, test, type Page } from 'playwright/test';
import { blocks, body, createDocument, openDocument, review, select, transaction } from './helpers/native-document';

async function turnInto(page: Page, id: string, label: string) {
  await select(page, id, 0, 'Before TARGET after.'.length);
  await page.getByRole('button', { name: 'Turn into', exact: true }).click();
  await page.getByRole('menuitem', { name: label, exact: true }).click();
}

for (const marker of ['-', '7.', '- [x]']) for (const entry of ['shortcut', 'autoformat', 'HTTP API']) {
  test(`${entry} converts ${marker} to H3 with the same native properties and comment target`, async ({ browser, request }) => {
    const fixture = await createDocument(request, `${marker} Before **TARGET** after.\n${marker} Second item.`);
    const [first, second] = await blocks(request, fixture.url);
    await transaction(request, fixture.url, [{ op: 'comment.add', block_id: first.block_id, start: 7, end: 13, body: 'Same characters' }], first.document_clock);
    const user = await openDocument(browser, fixture.path, 'Writer');
    try {
      await select(user.page, first.block_id, 0);
      if (entry === 'shortcut') await user.page.keyboard.press('ControlOrMeta+Alt+3');
      else if (entry === 'autoformat') await user.page.keyboard.type('### ');
      else await transaction(request, fixture.url, [{ op: 'set_block_type', block_id: first.block_id, block_type: 'h3' }], first.document_clock);
      const heading = body(user.page).locator(`h3[data-block-id="${first.block_id}"]`);
      await expect(heading).toHaveText(first.text);
      await expect(heading.locator('strong')).toHaveText('TARGET');
      await expect(heading.locator('[data-comment-id]')).toHaveText('TARGET');
      await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
      const result = (await (await request.get(`${fixture.url}/blocks`)).json()).blocks;
      expect(result[0]).toMatchObject({ block_id: first.block_id, block_type: 'h3', attrs: {}, text: first.text });
      expect(result[0].attrs).toEqual({});
      expect(result[1]).toMatchObject({ block_id: second.block_id, block_type: 'p', text: second.text });
      expect(result[1].attrs.listStyleType).toBeTruthy();
      const comment = (await review(request, fixture.url)).comments[0];
      expect(comment.target.attachments[0]).toMatchObject({ owner: { kind: 'block', id: first.block_id }, quote: 'TARGET' });
      await user.page.reload(); await expect(heading).toHaveText(first.text);
      expect(user.errors).toEqual([]);
    } finally { await user.context.close(); }
  });
}

for (const mode of ['editing', 'suggesting']) for (const kind of ['Code', 'Heading 3']) {
  test(`${kind} conversion in ${mode} mode preserves code structure and accepts concurrent HTTP edits`, async ({ browser, request }) => {
    const fixture = await createDocument(request, kind === 'Code' ? '- [x] Before TARGET after.\n- [x] Next.' : '```\nFirst\nBefore TARGET after.\nLast\n```\n\nNext.');
    const original = await blocks(request, fixture.url);
    const target = original.find((block) => block.text === 'Before TARGET after.')!;
    await transaction(request, fixture.url, [{ op: 'comment.add', block_id: target.block_id, start: 7, end: 13, body: 'Keep the original text' }], target.document_clock);
    const user = await openDocument(browser, fixture.path, 'Reviewer');
    try {
      if (mode === 'suggesting') {
        await user.page.getByRole('button', { name: 'Document mode', exact: true }).click();
        await user.page.getByRole('menuitem', { name: 'Suggesting', exact: true }).click();
      }
      await turnInto(user.page, target.block_id, kind);
      if (mode === 'suggesting') {
        await expect.poll(async () => (await review(request, fixture.url)).suggestions.length).toBe(1);
        expect((await blocks(request, fixture.url)).map(({ block_type, text }) => ({ block_type, text }))).toEqual(original.map(({ block_type, text }) => ({ block_type, text })));
        await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
        await user.page.reload(); await user.page.getByRole('tab', { name: /Comments/ }).click();
        await transaction(request, fixture.url, [{ op: 'replace_block_content', block_id: target.block_id, text: 'Agent Before TARGET after.' }], target.document_clock);
        await expect(body(user.page)).toContainText('Agent Before TARGET after.');
        await user.page.getByTestId('suggestion-card').getByRole('button', { name: 'Accept suggestion', exact: true }).click();
      }
      const element = body(user.page).locator(`[data-block-id="${target.block_id}"]`);
      await expect(element).toHaveClass(kind === 'Code' ? /slate-code_line/ : /slate-h3/);
      await expect(element).toHaveText((mode === 'suggesting' ? 'Agent ' : '') + target.text);
      await expect(element.locator('[data-comment-id]')).toHaveText('TARGET');
      if (kind === 'Heading 3') await expect(body(user.page).locator('.slate-code_block')).toHaveCount(2);
      await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
      await user.page.reload(); await expect(element.locator('[data-comment-id]')).toHaveText('TARGET');
      expect(user.errors).toEqual([]);
    } finally { await user.context.close(); }
  });
}

test('the HTTP conversion action handles code wrapping and leaving a list without explicit attributes', async ({ browser, request }) => {
  const fixture = await createDocument(request, '- [x] Before TARGET after.');
  let [target] = await blocks(request, fixture.url);
  const user = await openDocument(browser, fixture.path, 'Reader');
  try {
    for (const kind of ['p', 'code_block', 'h3']) {
      await transaction(request, fixture.url, [{ op: 'set_block_type', block_id: target.block_id, block_type: kind }], target.document_clock);
      [target] = (await blocks(request, fixture.url)).filter((block) => block.block_id === target.block_id);
      expect(target).toMatchObject({ block_type: kind === 'code_block' ? 'code_line' : kind, text: 'Before TARGET after.' });
      const block = body(user.page).locator(`[data-block-id="${target.block_id}"]`);
      await expect(block).toHaveClass(new RegExp(`slate-${kind === 'code_block' ? 'code_line' : kind}(?:\\s|$)`));
      await expect(body(user.page).getByRole('checkbox')).toHaveCount(0);
    }
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});
