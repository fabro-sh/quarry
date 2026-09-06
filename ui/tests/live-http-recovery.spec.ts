import { readFile } from 'node:fs/promises';
import { expect, test } from 'playwright/test';
import { blocks, body, createDocument, openDocument, select, transaction } from './helpers/native-document';

test('offline typing rejected after an agent deletion remains downloadable and recoverable after reload', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'TARGET\n\nSurvivor');
  const [target, survivor] = await blocks(request, fixture.url);
  const user = await openDocument(browser, fixture.path, 'Writer');
  try {
    await user.context.setOffline(true);
    await select(user.page, target.block_id, 0);
    await user.page.keyboard.insertText('Unsent browser ');
    await expect(body(user.page)).toContainText('Unsent browser TARGET');
    await expect(user.page.getByRole('button', { name: 'Download draft', exact: true })).toBeVisible();
    await transaction(request, fixture.url, [{ op: 'delete_block', block_id: target.block_id }], target.document_clock);
    await user.context.setOffline(false);
    await expect(user.page.getByText('Some changes could not be applied. Your draft is kept on this device.', { exact: true })).toBeVisible();
    expect((await blocks(request, fixture.url)).map((block) => block.text)).toEqual(['Survivor']);

    await user.page.reload();
    await expect(user.page.getByRole('button', { name: 'Use saved version', exact: true })).toBeVisible();
    const downloading = user.page.waitForEvent('download');
    await user.page.getByRole('button', { name: 'Download draft', exact: true }).click();
    const downloaded = await downloading;
    const recovery = JSON.parse(await readFile((await downloaded.path())!, 'utf8'));
    expect(recovery.drafts).toHaveLength(1);
    expect(recovery.drafts[0].state).toBe('failed');

    // Load the actual saved native bytes through a separate server authority.
    // A nonempty download alone would not prove the unsent text survived.
    const recoveredUrl = fixture.url.replace(/doc\.md$/, 'recovered.md');
    const imported = await request.post(`${recoveredUrl}/archive`, { data: {
      format: 'quarry-document', version: 1, metadata: {}, bytes: recovery.drafts[0].draft,
    } });
    expect(imported.ok(), await imported.text()).toBe(true);
    expect((await blocks(request, recoveredUrl)).map((block) => block.text)).toEqual(['Unsent browser TARGET', 'Survivor']);

    await user.page.getByRole('button', { name: 'Use saved version', exact: true }).click();
    await expect(body(user.page)).toHaveText('Survivor');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await select(user.page, survivor.block_id, 8);
    await user.page.keyboard.type('!');
    await expect.poll(async () => (await blocks(request, fixture.url))[0].text).toBe('Survivor!');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});
