import { expect, test } from 'playwright/test';
import { blocks, body, createDocument, openDocument, select, transaction } from './helpers/native-document';

test('a pending image upload follows its insertion block through an agent move', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Before TARGET\n\nMiddle\n\nLast');
  const initial = await blocks(request, fixture.url), target = initial[0];
  await transaction(request, fixture.url, [{ op: 'comment.add', block_id: target.block_id, start: 7, end: 13, body: 'Keep target' }], target.document_clock);
  const user = await openDocument(browser, fixture.path, 'Writer');
  let release!: () => void;
  const pending = new Promise<void>((resolve) => { release = resolve; });
  await user.context.route('**/documents/assets/**', async (route) => {
    if (route.request().method() === 'PUT') await pending;
    await route.continue();
  });
  try {
    await select(user.page, target.block_id, target.text.length);
    await body(user.page).evaluate((element) => {
      const bytes = Uint8Array.from(atob('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP43+DwHwAHAAK/K9fH4gAAAABJRU5ErkJggg=='), (value) => value.charCodeAt(0));
      const data = new DataTransfer(); data.items.add(new File([bytes], 'delayed.png', { type: 'image/png' }));
      const event = new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true });
      Object.defineProperty(event, 'clipboardData', { value: data }); element.dispatchEvent(event);
    });
    await expect(body(user.page).getByTestId('image-placeholder')).toBeVisible();
    expect((await blocks(request, fixture.url)).map((block) => block.block_type)).toEqual(['p', 'p', 'p']);
    await transaction(request, fixture.url, [
      { op: 'move_block', block_id: target.block_id, position: 2 },
      { op: 'replace_block_content', block_id: target.block_id, text: 'Agent Before TARGET' },
    ], target.document_clock);
    await expect(body(user.page)).toContainText('Agent Before TARGET');
    release();
    await expect(body(user.page).getByRole('img', { name: 'delayed.png' })).toBeVisible();
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    const saved = await blocks(request, fixture.url);
    expect(saved.map((block) => block.block_type)).toEqual(['p', 'p', 'p', 'img']);
    expect(saved.at(-2)?.block_id).toBe(target.block_id);
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    await user.page.reload();
    await expect(body(user.page).getByRole('img', { name: 'delayed.png' })).toBeVisible();
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    expect(user.errors).toEqual([]);
  } finally { release(); await user.context.close(); }
});
