import { expect, test } from 'playwright/test';
import { blocks, body, createDocument, openDocument, review, select, transaction } from './helpers/native-document';

for (const key of ['Backspace', 'Delete']) {
  test(`${key} extends one deletion across marked text, concurrent agent edits and reload`, async ({ browser, request }) => {
    const fixture = await createDocument(request, 'Before **TARGET** after.');
    const [initial] = await blocks(request, fixture.url);
    const user = await openDocument(browser, fixture.path, 'Reviewer');
    try {
      await user.page.getByRole('button', { name: 'Document mode', exact: true }).click();
      await user.page.getByRole('menuitem', { name: 'Suggesting', exact: true }).click();
      await select(user.page, initial.block_id, key === 'Backspace' ? 13 : 7);
      for (let i = 0; i < 3; i++) await user.page.keyboard.press(key);
      await expect.poll(async () => (await review(request, fixture.url)).suggestions.map((s) => s.quote)).toEqual([key === 'Backspace' ? 'GET' : 'TAR']);
      const id = (await review(request, fixture.url)).suggestions[0].id;
      const [read] = await blocks(request, fixture.url);
      await transaction(request, fixture.url, [
        { op: 'replace_block_content', block_id: read.block_id, text: `Agent ${read.text}`, marks: [{ start: 13, end: 19, marks: { bold: true } }] },
        { op: 'comment.add', block_id: read.block_id, start: 13, end: 19, body: 'Keep this discussion' },
      ], read.document_clock);
      await expect(body(user.page)).toContainText('Agent Before');
      for (let i = 0; i < 3; i++) await user.page.keyboard.press(key);
      await expect.poll(async () => (await review(request, fixture.url)).suggestions.map((s) => ({ id: s.id, quote: s.quote }))).toEqual([{ id, quote: 'TARGET' }]);
      expect((await blocks(request, fixture.url))[0].text).toBe('Agent Before TARGET after.');
      await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
      await user.page.reload();
      await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
      await user.page.getByRole('tab', { name: /Comments/ }).click();
      await expect(user.page.getByTestId('suggestion-card')).toHaveCount(1);
      await user.page.getByTestId('suggestion-card').getByRole('button', { name: 'Accept suggestion', exact: true }).click();
      await expect.poll(async () => (await blocks(request, fixture.url))[0].text).toBe('Agent Before  after.');
      const comments = (await review(request, fixture.url)).comments;
      expect(comments).toHaveLength(1);
      expect(comments[0].body).toBe('Keep this discussion');
      expect(user.errors).toEqual([]);
    } finally { await user.context.close(); }
  });
}

test('selection deletion continues through Unicode and can be rejected as one suggestion', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Before 😀TARGET after.');
  const [initial] = await blocks(request, fixture.url);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await user.page.getByRole('button', { name: 'Document mode', exact: true }).click();
    await user.page.getByRole('menuitem', { name: 'Suggesting', exact: true }).click();
    await select(user.page, initial.block_id, 12, 15);
    await user.page.keyboard.press('Backspace');
    for (let i = 0; i < 4; i++) await user.page.keyboard.press('Backspace');
    await expect.poll(async () => (await review(request, fixture.url)).suggestions.map((s) => s.quote)).toEqual(['😀TARGET']);
    expect((await blocks(request, fixture.url))[0].text).toBe(initial.text);
    await user.page.getByTestId('suggestion-card').getByRole('button', { name: 'Reject suggestion', exact: true }).click();
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload();
    await expect(body(user.page)).toHaveText(initial.text);
    await expect(body(user.page).locator('[data-suggestion-id]')).toHaveCount(0);
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('a delayed deletion extension preserves an unrelated agent suggestion', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Before TARGET after.\n\nAnother paragraph.');
  const [first, second] = await blocks(request, fixture.url);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  let held = false;
  try {
    await user.page.getByRole('button', { name: 'Document mode', exact: true }).click();
    await user.page.getByRole('menuitem', { name: 'Suggesting', exact: true }).click();
    await select(user.page, first.block_id, 13);
    await user.page.keyboard.press('Backspace');
    await expect.poll(async () => (await review(request, fixture.url)).suggestions.map((s) => s.quote)).toEqual(['T']);
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    const id = (await review(request, fixture.url)).suggestions[0].id;
    await user.page.route('**/document-commands', async (route) => {
      held = true; await gate; await route.continue();
    });
    await user.page.keyboard.press('Backspace'); await user.page.keyboard.press('Backspace');
    await expect.poll(() => held).toBe(true);
    const [read] = await blocks(request, fixture.url);
    await transaction(request, fixture.url, [{ op: 'suggestion.add', block_id: second.block_id, start: 0, end: 7, replacement: 'Updated', body: 'Independent review' }], read.document_clock);
    release();
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await expect.poll(async () => (await review(request, fixture.url)).suggestions.find((s) => s.id === id)?.quote).toBe('GET');
    const suggestions = (await review(request, fixture.url)).suggestions;
    expect(suggestions).toHaveLength(2);
    expect(suggestions.find((s) => s.id !== id)?.content).toBe('Updated');
    await user.page.reload();
    await expect(body(user.page).locator(`[data-suggestion-id="${id}"]`)).toHaveText('GET');
    expect((await blocks(request, fixture.url)).map((b) => b.text)).toEqual([first.text, second.text]);
    expect(user.errors).toEqual([]);
  } finally { release(); await user.context.close(); }
});


test('selection, mode changes, and reload start separate adjacent deletion intents', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Before TARGET after.');
  const [initial] = await blocks(request, fixture.url);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  const mode = async (name: string) => {
    await user.page.getByRole('button', { name: 'Document mode', exact: true }).click();
    await user.page.getByRole('menuitem', { name, exact: true }).click();
  };
  try {
    await mode('Suggesting'); await select(user.page, initial.block_id, 13);
    await user.page.keyboard.press('Backspace');
    await expect.poll(async () => (await review(request, fixture.url)).suggestions.map((s) => s.quote)).toEqual(['T']);
    await select(user.page, initial.block_id, 12);
    await user.page.keyboard.press('Backspace');
    await expect.poll(async () => (await review(request, fixture.url)).suggestions.map((s) => s.quote).sort()).toEqual(['E', 'T']);
    await mode('Editing'); await mode('Suggesting');
    await select(user.page, initial.block_id, 11); await user.page.keyboard.press('Backspace');
    await expect.poll(async () => (await review(request, fixture.url)).suggestions.map((s) => s.quote).sort()).toEqual(['E', 'G', 'T']);
    await user.page.reload();
    await expect(body(user.page).locator(`[data-block-id="${initial.block_id}"]`)).toBeVisible();
    await mode('Suggesting'); await select(user.page, initial.block_id, 10); await user.page.keyboard.press('Backspace');
    await expect.poll(async () => (await review(request, fixture.url)).suggestions.map((s) => s.quote).sort()).toEqual(['E', 'G', 'R', 'T']);
    expect((await blocks(request, fixture.url))[0].text).toBe(initial.text);
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('Suggesting composition keeps one native replacement while an agent edits and comments', async ({ browser, browserName, request }) => {
  const fixture = await createDocument(request, 'Before TARGET after.');
  const [initial] = await blocks(request, fixture.url);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await user.page.getByRole('button', { name: 'Document mode', exact: true }).click();
    await user.page.getByRole('menuitem', { name: 'Suggesting', exact: true }).click();
    await select(user.page, initial.block_id, 7, 13);
    const ime = browserName === 'chromium' ? await user.context.newCDPSession(user.page) : undefined;
    if (ime) await ime.send('Input.imeSetComposition', { text: '你', selectionStart: 1, selectionEnd: 1 });
    else await body(user.page).dispatchEvent('compositionstart', { data: '' });
    await transaction(request, fixture.url, [
      { op: 'replace_block_content', block_id: initial.block_id, text: 'Agent ' + initial.text },
      { op: 'comment.add', block_id: initial.block_id, start: 13, end: 19, body: 'Keep the original discussion' },
    ], initial.document_clock);
    await user.page.waitForTimeout(1800);
    if (ime) {
      await ime.send('Input.imeSetComposition', { text: '你好', selectionStart: 2, selectionEnd: 2 });
      await ime.send('Input.insertText', { text: '你好' });
    } else {
      await user.page.keyboard.insertText('你好');
      await body(user.page).dispatchEvent('compositionend', { data: '你好' });
    }
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await expect.poll(async () => (await review(request, fixture.url)).suggestions.map((s) => ({ quote: s.quote, content: s.content }))).toEqual([{ quote: 'TARGET', content: '你好' }]);
    expect((await blocks(request, fixture.url))[0].text).toBe('Agent Before TARGET after.');
    await user.page.reload();
    await expect(body(user.page)).toContainText('你好');
    expect((await review(request, fixture.url)).comments[0].body).toBe('Keep the original discussion');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});
