import { test, expect } from 'playwright/test';
import { blocks, body, createDocument, openDocument, select, transaction } from './helpers/native-document';

// Slate #5988: Strict Mode's second render must not clear a chunk's pending
// update. Small documents do not exercise the memoized chunk rendering path.
test('chunked text and highlights render every edit across distant blocks', async ({ browser, request }) => {
  test.setTimeout(120000);
  const fixture = await createDocument(request, '# Performance\n\n' + Array.from({ length: 1400 }, (_, i) => `Paragraph ${i}: ${'content '.repeat(8)}`).join('\n\n'));
  const user = await openDocument(browser, fixture.path, 'Writer');
  const initial = await blocks(request, fixture.url);
  try {
    expect(await body(user.page).locator('[data-slate-chunk]').count()).toBeGreaterThan(0);
    for (const index of [0, 1000, 1400]) {
      const block = initial[index];
      const element = body(user.page).locator(`[data-block-id="${block.block_id}"]`);
      await element.scrollIntoViewIfNeeded();
      await select(user.page, block.block_id, 0);
      // Check each committed render, especially the first key in a chunk.
      for (const [step, key] of [...'abc'].entries()) {
        await user.page.keyboard.type(key);
        await expect(element).toHaveText('abc'.slice(0, step + 1) + block.text);
      }
      // This rendering test uses the debug server. The release performance
      // suite separately enforces save and input budgets on this large fixture.
      await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved', { timeout: 60000 });
      const current = (await blocks(request, fixture.url))[index];
      await transaction(request, fixture.url, [{ op: 'comment.add', block_id: current.block_id, start: 3, end: 12, body: `Chunk ${index}` }], current.document_clock);
      await expect(element.locator('[data-comment-id]')).toHaveText(block.text.slice(0, 9));
      await select(user.page, block.block_id, 0);
      await user.page.keyboard.type('!');
      await expect(element).toHaveText('!abc' + block.text);
      await expect(element.locator('[data-comment-id]')).toHaveText(block.text.slice(0, 9));
    }
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved', { timeout: 60000 });
    await user.page.reload();
    for (const index of [0, 1000, 1400]) {
      const block = initial[index];
      const element = body(user.page).locator(`[data-block-id="${block.block_id}"]`);
      await expect(element).toHaveText('!abc' + block.text);
      await expect(element.locator('[data-comment-id]')).toHaveText(block.text.slice(0, 9));
    }
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});
