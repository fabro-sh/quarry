import { expect, test } from 'playwright/test';
import { blocks, body, createDocument, openDocument, review, select, selectRange, transaction } from './helpers/native-document';

test.describe.configure({ timeout: 60_000 });

async function setSuggesting(page: import('playwright/test').Page) {
  await page.getByRole('button', { name: 'Document mode', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Suggesting', exact: true }).click();
}

async function pasteClipboard(page: import('playwright/test').Page, entries: [string, string][]) {
  await body(page).evaluate((element, entries) => {
    const data = new DataTransfer();
    for (const [type, value] of entries) data.setData(type, value);
    const event = new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true });
    Object.defineProperty(event, 'clipboardData', { value: data });
    element.dispatchEvent(event);
    if (!event.defaultPrevented) {
      const input = new InputEvent('beforeinput', { inputType: 'insertFromPaste', bubbles: true, cancelable: true });
      Object.defineProperty(input, 'dataTransfer', { value: data });
      element.dispatchEvent(input);
    }
  }, entries);
}

test('suggested paragraph splits and joins stay structural through acceptance and agent edits', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Before TARGET after.\n\nSecond paragraph.');
  const initial = await blocks(request, fixture.url);
  await transaction(request, fixture.url, [{
    op: 'comment.add', block_id: initial[0].block_id, start: 7, end: 13, body: 'Keep identity',
  }], initial[0].document_clock);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await setSuggesting(user.page);
    await select(user.page, initial[0].block_id, 7);
    await user.page.keyboard.press('Enter');
    await user.page.keyboard.type('New ');
    await expect.poll(async () => (await review(request, fixture.url)).suggestions.map((item) => item.kind)).toEqual(['block_update']);
    expect((await blocks(request, fixture.url)).map((block) => block.text)).toEqual(initial.map((block) => block.text));
    await transaction(request, fixture.url, [{
      op: 'replace_block_content', block_id: initial[0].block_id, text: 'Before TAR!GET after.',
    }], (await blocks(request, fixture.url))[0].document_clock);
    await expect(body(user.page)).toContainText('TAR!GET');
    await user.page.getByTestId('suggestion-card').getByRole('button', { name: 'Accept suggestion', exact: true }).click();
    await expect.poll(async () => (await blocks(request, fixture.url)).map((block) => block.text)).toEqual([
      'Before ', 'New TAR!GET after.', 'Second paragraph.',
    ]);
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TAR!GET');

    const current = await blocks(request, fixture.url);
    await select(user.page, current[1].block_id, 0);
    await user.page.keyboard.press('Backspace');
    await expect.poll(async () => (await review(request, fixture.url)).suggestions.filter((item) => item.status === 'open').length).toBe(1);
    expect((await blocks(request, fixture.url)).map((block) => block.text)).toEqual(current.map((block) => block.text));
    await user.page.getByTestId('suggestion-card').getByRole('button', { name: 'Accept suggestion', exact: true }).click();
    await expect.poll(async () => (await blocks(request, fixture.url)).map((block) => block.text)).toEqual([
      'Before New TAR!GET after.', 'Second paragraph.',
    ]);
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TAR!GET');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('a browser selection can replace canonical and proposed text in one suggestion', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Before TARGET after.');
  const [block] = await blocks(request, fixture.url);
  await transaction(request, fixture.url, [{
    op: 'suggestion.add', block_id: block.block_id,
    start: 7, end: 7, replacement: 'NEW', body: '',
  }], block.document_clock);
  const agentInsertion = (await review(request, fixture.url)).suggestions[0].id;
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await setSuggesting(user.page);
    await selectRange(user.page, { id: block.block_id, offset: 4 }, { id: block.block_id, offset: 10 });
    await user.page.keyboard.type('Changed');
    await expect.poll(async () => {
      const state = await review(request, fixture.url);
      return state.suggestions.map((item) => [item.id, item.status, item.content]);
    }).toEqual(expect.arrayContaining([
      [agentInsertion, 'resolved', ''],
      [expect.any(String), 'open', 'Changed'],
    ]));
    expect((await blocks(request, fixture.url))[0].text).toBe('Before TARGET after.');
    await user.page.getByTestId('suggestion-card').getByRole('button', { name: 'Accept suggestion', exact: true }).click();
    await expect.poll(async () => (await blocks(request, fixture.url))[0].text).toBe('BefoChangedGET after.');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('direct editing can replace blocks and proposed text in one native change', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Before TARGET\n\nSecond END');
  const initial = await blocks(request, fixture.url);
  await transaction(request, fixture.url, [
    { op: 'suggestion.add', block_id: initial[0].block_id, start: 7, end: 7, replacement: 'NEW', body: '' },
    { op: 'comment.add', block_id: initial[1].block_id, start: 7, end: 10, body: 'Keep identity' },
  ], initial[0].document_clock);
  const insertion = (await review(request, fixture.url)).suggestions[0].id;
  const user = await openDocument(browser, fixture.path, 'Writer');
  try {
    await selectRange(user.page, { id: initial[0].block_id, offset: 4 }, { id: initial[1].block_id, offset: 6 });
    await user.page.keyboard.type('Changed');
    await expect.poll(async () => (await blocks(request, fixture.url)).map((block) => block.text)).toEqual(['BefoChanged END']);
    await expect.poll(async () => (await review(request, fixture.url)).suggestions.find((item) => item.id === insertion)?.status).toBe('resolved');
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('END');
    expect((await review(request, fixture.url)).comments[0].target.attachments[0]).toMatchObject({
      owner: { kind: 'block', id: initial[0].block_id }, quote: 'END',
    });
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('real cut and paste events move comment identity across sessions once and repeat as a fresh copy', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'AA TARGET ZZ\n\ndestination');
  const initial = await blocks(request, fixture.url);
  await transaction(request, fixture.url, [{
    op: 'comment.add', block_id: initial[0].block_id, start: 3, end: 9, body: 'Keep identity',
  }], initial[0].document_clock);
  const user = await openDocument(browser, fixture.path, 'Writer');
  let receiver: Awaited<ReturnType<typeof openDocument>> | undefined;
  try {
    await select(user.page, initial[0].block_id, 3, 9);
    const cutData = await body(user.page).evaluate((element) => {
      const data = new DataTransfer();
      const event = new ClipboardEvent('cut', { clipboardData: data, bubbles: true, cancelable: true });
      Object.defineProperty(event, 'clipboardData', { value: data });
      element.dispatchEvent(event);
      return [...data.types].map((type) => [type, data.getData(type)] as [string, string]);
    });
    expect(cutData.map(([type]) => type)).toContain('application/x-quarry-cut');
    await expect.poll(async () => (await blocks(request, fixture.url))[0].text).toBe('AA  ZZ');
    await expect.poll(async () => (await review(request, fixture.url)).comments[0].target.state).toBe('hidden');

    receiver = await openDocument(browser, fixture.path, 'Other writer');
    await select(receiver.page, initial[1].block_id, 11);
    await pasteClipboard(receiver.page, cutData);
    await expect.poll(async () => (await blocks(request, fixture.url))[1].text).toBe('destinationTARGET');
    await expect.poll(async () => (await review(request, fixture.url)).comments[0].target.state).toBe('attached');
    await expect(body(receiver.page).locator('[data-comment-id]')).toHaveText('TARGET');

    await pasteClipboard(receiver.page, cutData);
    await expect.poll(async () => (await blocks(request, fixture.url))[1].text).toBe('destinationTARGETTARGET');
    expect((await review(request, fixture.url)).comments[0].target.attachments).toHaveLength(1);
    await receiver.page.reload();
    await expect(body(receiver.page).locator('[data-comment-id]')).toHaveText('TARGET');
    expect(user.errors).toEqual([]); expect(receiver.errors).toEqual([]);
  } finally { await receiver?.context.close(); await user.context.close(); }
});

test('real copy and paste events create a native suggestion', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Copy TARGET.\n\nDestination.');
  const initial = await blocks(request, fixture.url);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await select(user.page, initial[0].block_id, 5, 11);
    const copyData = await body(user.page).evaluate((element) => {
      const data = new DataTransfer();
      const event = new ClipboardEvent('copy', { clipboardData: data, bubbles: true, cancelable: true });
      Object.defineProperty(event, 'clipboardData', { value: data });
      element.dispatchEvent(event);
      return [...data.types].map((type) => [type, data.getData(type)] as [string, string]);
    });
    await setSuggesting(user.page);
    await select(user.page, initial[1].block_id, 5);
    await pasteClipboard(user.page, copyData);
    await expect.poll(async () => (await review(request, fixture.url)).suggestions.filter((item) => item.status === 'open').map((item) => item.content)).toEqual(['TARGET']);
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('real multi-block copy and paste is one structural suggestion', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Destination TARGET tail.\n\nCopy one.\n\nCopy two.');
  const initial = await blocks(request, fixture.url);
  await transaction(request, fixture.url, [{
    op: 'comment.add', block_id: initial[0].block_id, start: 12, end: 18, body: 'Keep destination identity',
  }], initial[0].document_clock);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await selectRange(user.page, { id: initial[1].block_id, offset: 0 }, { id: initial[2].block_id, offset: initial[2].text.length });
    const copyData = await body(user.page).evaluate((element) => {
      const data = new DataTransfer();
      const event = new ClipboardEvent('copy', { clipboardData: data, bubbles: true, cancelable: true });
      Object.defineProperty(event, 'clipboardData', { value: data }); element.dispatchEvent(event);
      return [...data.types].map((type) => [type, data.getData(type)] as [string, string]);
    });
    await setSuggesting(user.page);
    await select(user.page, initial[0].block_id, 12);
    await pasteClipboard(user.page, copyData);
    await expect.poll(async () => {
      const open = (await review(request, fixture.url)).suggestions.filter((item) => item.status === 'open');
      return open.map((item) => [item.kind, item.action?.kind]);
    }).toEqual([['block_update', 'paste_blocks']]);
    expect((await blocks(request, fixture.url)).map((block) => block.text)).toEqual(initial.map((block) => block.text));
    await user.page.getByTestId('suggestion-card').getByRole('button', { name: 'Accept suggestion', exact: true }).click();
    await expect.poll(async () => (await blocks(request, fixture.url)).map((block) => block.text)).toEqual([
      'Destination Copy one.', 'Copy two.TARGET tail.', 'Copy one.', 'Copy two.',
    ]);
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});
