import { expect, test } from 'playwright/test';
import { apiOrigin, blocks, body, createDocument, openDocument, select, transaction, review } from './helpers/native-document';

test('workspace create, keyboard rename, search, delete and title refresh use the native authority', async ({ browser, request }) => {
  const fixture = await createDocument(request, '# Original title\n\nWorkspace text');
  const user = await openDocument(browser, fixture.path, 'Writer');
  try {
    await expect(user.page).toHaveTitle('Original title · Quarry');
    const first = (await blocks(request, fixture.url))[0];
    await select(user.page, first.block_id, 0); await user.page.keyboard.type('Updated ');
    await expect(user.page).toHaveTitle('Updated Original title · Quarry');
    await user.page.getByRole('button', { name: 'Create document', exact: true }).click({ trial: true });
    user.page.once('dialog', (dialog) => dialog.accept('created.md'));
    await user.page.getByRole('button', { name: 'Create document', exact: true }).click();
    await expect(user.page).toHaveURL(/created.md$/); await expect(body(user.page)).toContainText('Untitled');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    const tree = user.page.getByRole('treeitem').filter({ hasText: 'created.md' });
    await tree.click(); await tree.focus();
    user.page.once('dialog', (dialog) => dialog.accept('renamed.md'));
    await user.page.keyboard.press('F2');
    await expect(user.page).toHaveURL(/renamed.md$/);
    const renamed = `${apiOrigin}/v1/libraries/${fixture.library}/documents/renamed.md`;
    await expect.poll(async () => (await request.get(renamed)).status()).toBe(200);
    await user.page.reload(); await expect(body(user.page)).toContainText('Untitled');
    await user.page.getByRole('treeitem').filter({ hasText: 'renamed.md' }).click({ button: 'right' });
    user.page.once('dialog', (dialog) => dialog.accept());
    await user.page.getByRole('menuitem', { name: /Delete/ }).click();
    await expect.poll(async () => (await request.get(renamed)).status()).toBe(404);
    await expect(user.page.getByRole('treeitem').filter({ hasText: 'renamed.md' })).toHaveCount(0);
    await user.page.getByRole('button', { name: 'Search', exact: true }).click();
    await user.page.getByRole('textbox', { name: 'Search', exact: true }).fill('Workspace text');
    await expect(user.page.getByRole('option').filter({ hasText: 'doc.md' })).toBeVisible();
    await user.page.getByRole('listbox', { name: 'Search results' }).focus();
    await user.page.keyboard.press('Enter');
    await expect(body(user.page)).toContainText('Workspace text');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('historical restore updates the open editor while native review survives', async ({ browser, request }) => {
  const fixture = await createDocument(request, '# History\n\nOriginal TARGET');
  const initial = await request.get(fixture.url); const first = (await blocks(request, fixture.url))[1];
  const state = await (await request.get(`${fixture.url}/document-state`)).json();
  const user = await openDocument(browser, fixture.path, 'Writer');
  try {
    await transaction(request, fixture.url, [{ op: 'comment.add', block_id: first.block_id, start: 9, end: 15, body: 'Keep this review' }], first.document_clock);
    await select(user.page, first.block_id, 0); await user.page.keyboard.type('Later ');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.getByRole('tab', { name: 'Versions', exact: true }).click();
    const id = state.document_clock;
    await user.page.getByRole('button', { name: `Restore version ${id}`, exact: true }).click();
    await expect(body(user.page)).toContainText('Original TARGET');
    await expect(body(user.page)).not.toContainText('Later ');
    const retained = await review(request, fixture.url);
    expect(JSON.stringify(retained)).toContain('Keep this review');
    expect(JSON.stringify(retained)).toContain('TARGET');
    expect((await request.get(fixture.url)).headers().etag).not.toBe(initial.headers().etag);
    await user.page.reload(); await expect(body(user.page)).not.toContainText('Later ');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});
