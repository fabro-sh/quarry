import { expect, test } from 'playwright/test';
import AxeBuilder from '@axe-core/playwright';
import { blocks, body, createDocument, openDocument, select, transaction, uiOrigin } from './helpers/native-document';

test.describe.configure({ timeout: 60000 });

test('an interrupted engine download can retry and save without reloading the page', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Download recovery');
  const context = await browser.newContext();
  try {
    await context.addInitScript(() => localStorage.setItem('quarry:author', 'Writer'));
    let downloads = 0;
    await context.route(/\/quarry_document_bg(?:-[\w-]+)?\.wasm(?:\?.*)?$/, (route) => ++downloads === 1 ? route.abort() : route.continue());
    const page = await context.newPage();
    await page.goto(`${uiOrigin}${fixture.path}`);
    await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Try again', exact: true }).click();
    await expect(body(page)).toHaveText('Download recovery');
    expect(downloads).toBe(2);
    await body(page).click(); await page.keyboard.press('End'); await page.keyboard.type(' works');
    await expect(page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await expect.poll(async () => (await blocks(request, fixture.url))[0].text).toBe('Download recovery works');
  } finally { await context.close(); }
});

test('keyboard shortcuts, task checkboxes and table alignment persist with native targets', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'TARGET\n\nTask\n\n| A | B |\n| --- | --- |\n| C | D |\n');
  const initial = await blocks(request, fixture.url); const target = initial.find((v) => v.text === 'TARGET')!;
  await transaction(request, fixture.url, [{ op: 'comment.add', block_id: target.block_id, start: 0, end: 6, body: 'Keep target' }], target.document_clock);
  const user = await openDocument(browser, fixture.path, 'Writer');
  try {
    await select(user.page, target.block_id, 0); await user.page.keyboard.type('# ');
    await expect(body(user.page).locator('h1')).toHaveText('TARGET');
    const task = initial.find((v) => v.text === 'Task')!;
    await select(user.page, task.block_id, 0); await user.page.keyboard.type('[x] ');
    await expect(body(user.page).getByRole('checkbox')).toBeChecked();
    await body(user.page).getByRole('checkbox').uncheck();
    const b = initial.find((v) => v.text === 'B')!;
    await select(user.page, b.block_id, 0);
    await body(user.page).locator('th').last().getByRole('button', { name: 'Column options', exact: true }).click();
    await user.page.getByRole('menuitem', { name: 'Align right', exact: true }).click();
    await expect(body(user.page).locator('th').last()).toHaveCSS('text-align', 'right');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await body(user.page).locator('th').first().locator('[data-slate-string]').click();
    const beforeTab = (await (await request.get(`${fixture.url}/versions`)).json()).length;
    await user.page.keyboard.press('Tab');
    await expect.poll(() => user.page.evaluate(() => getSelection()?.toString())).toBe('B');
    expect((await (await request.get(`${fixture.url}/versions`)).json()).length).toBe(beforeTab);
    // Plate selects the next cell. Collapse to its start to insert a prefix.
    await user.page.keyboard.press('ArrowLeft'); await user.page.keyboard.type('Next ');
    await expect.poll(async () => (await blocks(request, fixture.url)).find((v) => v.block_id === b.block_id)?.text).toBe('Next B');
    await user.page.reload(); await expect(body(user.page).locator('h1')).toHaveText('TARGET');
    await expect(body(user.page).getByRole('checkbox')).not.toBeChecked();
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    await expect(body(user.page).locator('th').last()).toHaveCSS('text-align', 'right');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('typing and undo after a trailing diagram preserve the diagram and do not save an input scaffold', async ({ browser, request }) => {
  const fixture = await createDocument(request, '```mermaid\nflowchart LR\nA --> B\n```\n');
  const user = await openDocument(browser, fixture.path, 'Writer');
  try {
    await body(user.page).locator('.slate-p').last().click(); await user.page.keyboard.insertText('After diagram');
    await expect.poll(async () => (await blocks(request, fixture.url)).map((v) => v.block_type)).toEqual(['mermaid', 'p']);
    await user.page.keyboard.press('ControlOrMeta+z');
    await expect.poll(async () => (await blocks(request, fixture.url)).length).toBe(1);
    await body(user.page).locator('.slate-p').last().click(); await user.page.keyboard.insertText('Second');
    await expect.poll(async () => (await blocks(request, fixture.url)).at(-1)?.text).toBe('Second');
    const paragraph = (await blocks(request, fixture.url)).at(-1)!;
    await select(user.page, paragraph.block_id, 0); await user.page.keyboard.press('Backspace');
    await expect.poll(async () => (await blocks(request, fixture.url)).map((v) => v.block_type)).toEqual(['p']);
    await user.page.reload(); await expect(body(user.page)).toHaveText('Second');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('composition retains local Chinese input while an agent edits the same paragraph', async ({ browser, browserName, request }) => {
  const fixture = await createDocument(request, 'TARGET\n'); const [block] = await blocks(request, fixture.url);
  const user = await openDocument(browser, fixture.path, 'Writer');
  try {
    await select(user.page, block.block_id, 3);
    const ime = browserName === 'chromium' ? await user.context.newCDPSession(user.page) : undefined;
    if (ime) await ime.send('Input.imeSetComposition', { text: '你', selectionStart: 1, selectionEnd: 1 });
    else await body(user.page).dispatchEvent('compositionstart', { data: '' });
    await transaction(request, fixture.url, [{ op: 'replace_block_content', block_id: block.block_id, text: 'Agent TARGET' }], block.document_clock);
    // Let the transport receive the remote change while the input is composing.
    await user.page.waitForTimeout(1800);
    await expect(body(user.page)).toHaveText(ime ? 'TAR你GET' : 'TARGET');
    if (ime) {
      await ime.send('Input.imeSetComposition', { text: '你好', selectionStart: 2, selectionEnd: 2 });
      await ime.send('Input.insertText', { text: '你好' });
    } else {
      await user.page.keyboard.insertText('你好');
      await body(user.page).dispatchEvent('compositionend', { data: '你好' });
    }
    await expect(body(user.page)).toHaveText('Agent TAR你好GET');
    await expect.poll(async () => (await blocks(request, fixture.url))[0].text).toBe('Agent TAR你好GET');
    await user.page.reload(); await expect(body(user.page)).toHaveText('Agent TAR你好GET');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('image uploads insert an asset and survive browser reload', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Before TARGET\n'); const [block] = await blocks(request, fixture.url);
  const user = await openDocument(browser, fixture.path, 'Writer');
  try {
    await select(user.page, block.block_id, 7);
    await body(user.page).evaluate((element) => {
      const bytes = Uint8Array.from(atob('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP43+DwHwAHAAK/K9fH4gAAAABJRU5ErkJggg=='), (character) => character.charCodeAt(0));
      const clipboardData = new DataTransfer();
      clipboardData.items.add(new File([bytes], 'pixel.png', { type: 'image/png' }));
      const event = new ClipboardEvent('paste', { clipboardData, bubbles: true, cancelable: true });
      Object.defineProperty(event, 'clipboardData', { value: clipboardData });
      element.dispatchEvent(event);
    });
    await expect(body(user.page).getByRole('img', { name: 'pixel.png' })).toBeVisible();
    await expect.poll(async () => (await blocks(request, fixture.url)).filter((v) => v.block_type === 'img').length).toBe(1);
    await user.page.reload(); await expect(body(user.page).getByRole('img', { name: 'pixel.png' })).toBeVisible();
    await expect.poll(() => body(user.page).getByRole('img', { name: 'pixel.png' }).evaluate((img) => (img as HTMLImageElement).naturalWidth)).toBe(1);
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('the document workspace and review controls pass automated accessibility checks', async ({ browser, request }) => {
  const fixture = await createDocument(request, '# Review\n\nTARGET\n');
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    const result = await new AxeBuilder({ page: user.page }).analyze();
    expect(result.violations.map(({ id, nodes }) => ({ id, targets: nodes.map((node) => node.target) }))).toEqual([]);
    await user.page.screenshot({ path: test.info().outputPath('document-workspace.png'), fullPage: true });
    await user.page.setViewportSize({ width: 390, height: 844 });
    expect(await user.page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});
