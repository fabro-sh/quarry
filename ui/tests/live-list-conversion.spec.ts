import { expect, test, type Page } from 'playwright/test';
import { blocks, body, createDocument, openDocument, review, select, transaction, type Block } from './helpers/native-document';

const styles = [
  { name: 'bullet', marker: '-', style: 'disc' },
  { name: 'numbered', marker: '7.', style: 'decimal' },
  { name: 'checked task', marker: '- [x]', style: 'todo' },
];
type Menu = 'toolbar' | 'block actions';
type ListBlock = Block & { attrs: Record<string, unknown> };

async function convert(page: Page, id: string, menu: Menu, label: string) {
  if (menu === 'toolbar') {
    await select(page, id, 0, 'Before TARGET after.'.length);
    await page.getByRole('button', { name: 'Turn into', exact: true }).click();
    await page.getByRole('menuitem', { name: label, exact: true }).click();
  } else {
    const block = body(page).locator(`[data-block-id="${id}"]`);
    await block.scrollIntoViewIfNeeded(); await block.hover();
    await block.locator('..').getByRole('button', { name: /^(Drag to move block|Block actions)$/ }).click();
    const actions = page.getByRole('menu', { name: 'Block actions', exact: true });
    await actions.getByRole('menuitem', { name: 'Turn into', exact: true }).hover();
    await actions.getByRole('menuitem', { name: label, exact: true }).click();
  }
}

for (const style of styles) for (const menu of ['toolbar', 'block actions'] as const) for (const mode of ['editing', 'suggesting']) {
  test(`${style.name} list to H3 through ${menu} in ${mode} mode retains comments and text`, async ({ browser, request }) => {
    const fixture = await createDocument(request, `${style.marker} Before **TARGET** after.\n${style.marker} Second item.`);
    const [first, second] = await blocks(request, fixture.url) as ListBlock[];
    expect(first.attrs.listStyleType).toBe(style.style);
    await transaction(request, fixture.url, [{ op: 'comment.add', block_id: first.block_id, start: 7, end: 13, body: 'Keep these characters' }], first.document_clock);
    const user = await openDocument(browser, fixture.path, 'Reviewer');
    const current = async () => (await blocks(request, fixture.url) as ListBlock[]).find((block) => block.block_id === first.block_id)!;
    const heading = body(user.page).locator(`h3[data-block-id="${first.block_id}"]`);
    try {
      if (mode === 'suggesting') {
        await user.page.getByRole('button', { name: 'Document mode', exact: true }).click();
        await user.page.getByRole('menuitem', { name: 'Suggesting', exact: true }).click();
      }
      await convert(user.page, first.block_id, menu, 'Heading 3');
      if (mode === 'suggesting') {
        await expect.poll(async () => (await review(request, fixture.url)).suggestions.length).toBe(1);
        expect(await current()).toMatchObject({ block_type: 'p', attrs: first.attrs, text: first.text });
        await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
        await user.page.reload();
        await user.page.getByRole('tab', { name: /Comments/ }).click();
        const card = user.page.getByTestId('suggestion-card');
        await expect(card).toHaveCount(1);
        await transaction(request, fixture.url, [{ op: 'replace_block_content', block_id: first.block_id, text: 'Agent Before TARGET after.' }], first.document_clock);
        await expect(body(user.page).locator(`[data-block-id="${first.block_id}"]`)).toContainText('Agent Before TARGET after.');
        await card.getByRole('button', { name: 'Accept suggestion', exact: true }).click();
      } else {
        await expect(heading).toHaveText(first.text);
        await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
        await body(user.page).focus(); await user.page.keyboard.press('ControlOrMeta+z');
        await expect.poll(async () => (await current()).attrs).toEqual(first.attrs);
        expect((await current()).block_type).toBe('p');
        await user.page.keyboard.press('ControlOrMeta+Shift+z');
      }
      const text = (mode === 'suggesting' ? 'Agent ' : '') + first.text;
      await expect(heading).toHaveText(text);
      await expect.poll(async () => ({ kind: (await current()).block_type, attrs: (await current()).attrs })).toEqual({ kind: 'h3', attrs: {} });
      expect((await blocks(request, fixture.url) as ListBlock[]).find((block) => block.block_id === second.block_id)?.attrs.listStyleType).toBe(style.style);
      expect(await heading.evaluate((element) => !!element.closest('li,ul,ol'))).toBe(false);
      await expect(heading.locator('strong')).toHaveText('TARGET');
      await expect(heading.locator('[data-comment-id]')).toHaveText('TARGET');
      const comment = (await review(request, fixture.url)).comments[0];
      expect(comment.target.attachments).toEqual([expect.objectContaining({ owner: { kind: 'block', id: first.block_id }, quote: 'TARGET' })]);
      await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
      await user.page.reload();
      await expect(heading).toHaveText(text);
      await expect(heading.locator('[data-comment-id]')).toHaveText('TARGET');
      expect(user.errors).toEqual([]);
    } finally { await user.context.close(); }
  });
}

for (const menu of ['toolbar', 'block actions'] as const) {
  test(`Text through ${menu} removes list membership even though the native kind remains p`, async ({ browser, request }) => {
    const fixture = await createDocument(request, '- [x] Before TARGET after.');
    const [first] = await blocks(request, fixture.url);
    const user = await openDocument(browser, fixture.path, 'Writer');
    try {
      await convert(user.page, first.block_id, menu, 'Text');
      await expect.poll(async () => (await blocks(request, fixture.url) as ListBlock[])[0].attrs).toEqual({});
      expect((await blocks(request, fixture.url))[0]).toMatchObject({ block_id: first.block_id, block_type: 'p', text: first.text });
      await expect(body(user.page).getByRole('checkbox')).toHaveCount(0);
      await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
      await user.page.reload();
      await expect(body(user.page)).toHaveText(first.text);
      expect(user.errors).toEqual([]);
    } finally { await user.context.close(); }
  });
}
