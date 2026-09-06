import { expect, test } from 'playwright/test';
import type { CommandBatch } from '../src/api/generated/schema/types.gen';
import { blocks, body, createDocument, openDocument, paste, review, selectRange, transaction } from './helpers/native-document';

for (const kind of ['paragraphs', 'list', 'headings', 'code']) {
  for (const action of ['Backspace', 'Delete', 'type', 'paste', 'insertText']) {
    test(`${action} across three ${kind} preserves surviving comments, formatting and undo`, async ({ browser, request }) => {
      const markdown = kind === 'code' ? '```\n😀 Keep FIRST\nRemove middle\nLAST TARGET\n```'
        : kind === 'list' ? '- 😀 Keep FIRST\n- Remove middle\n- LAST **TARGET**'
        : kind === 'headings' ? '## 😀 Keep FIRST\n\nRemove middle\n\n### LAST **TARGET**'
        : '😀 Keep FIRST\n\nRemove middle\n\nLAST **TARGET**';
      const fixture = await createDocument(request, markdown);
      const initial = await blocks(request, fixture.url);
      const [first, middle, last] = initial.filter((block) => block.block_type !== 'code_block');
      await transaction(request, fixture.url, [
        { op: 'comment.add', block_id: last.block_id, start: 5, end: 11, body: 'Surviving target' },
        { op: 'comment.add', block_id: middle.block_id, start: 0, end: middle.text.length, body: 'Removed target' },
      ], first.document_clock);
      const user = await openDocument(browser, fixture.path, 'Writer');
      try {
        const publications: CommandBatch[] = [];
        user.page.on('request', (request) => {
          if (request.method() === 'POST' && request.url().includes('/document-commands')) publications.push(request.postDataJSON());
        });
        await selectRange(user.page, { id: first.block_id, offset: 8 }, { id: last.block_id, offset: 5 });
        if (action === 'type') await user.page.keyboard.press('X');
        else if (action === 'insertText') await user.page.keyboard.insertText('New ');
        else if (action === 'paste') await paste(user.page, '<p>New </p>', 'New ');
        else await user.page.keyboard.press(action);
        const expected = `😀 Keep ${action === 'type' ? 'X' : action === 'paste' || action === 'insertText' ? 'New ' : ''}TARGET`;
        const textBlocks = async () => (await blocks(request, fixture.url)).filter((block) => block.block_type !== 'code_block');
        await expect.poll(async () => (await textBlocks()).map((block) => block.text)).toEqual([expected]);
        await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
        const requests = publications.flatMap((batch) => batch.requests);
        // Firefox implements the automation insertText call as IME input.
        // Composition publishes complete updates with one undo group; ordinary
        // keypresses, deletion and paste must each publish one native request.
        if (action !== 'insertText') expect(requests).toHaveLength(1);
        expect(requests.flatMap((request) => request.commands).filter((command) => command.op === 'edit' && command.action.op === 'replace_selection')).toHaveLength(1);
        if (kind !== 'code') await expect(body(user.page).locator('strong')).toHaveText('TARGET');
        const comments = (await review(request, fixture.url)).comments;
        expect(comments.find((comment) => comment.body === 'Surviving target').target.attachments[0]).toMatchObject({ owner: { id: first.block_id }, quote: 'TARGET' });
        expect(comments.find((comment) => comment.body === 'Removed target').target.state).toBe('hidden');
        // A comment written from the earlier API read must find the original
        // characters after their block was absorbed by the join.
        await transaction(request, fixture.url, [{ op: 'comment.add', block_id: last.block_id, start: 5, end: 11, body: 'Late target' }], first.document_clock);
        await expect(body(user.page).locator('[data-comment-id]')).toContainText(['TARGET']);
        await body(user.page).focus(); await user.page.keyboard.press('ControlOrMeta+z');
        await expect.poll(async () => (await textBlocks()).map((block) => block.text)).toEqual([first.text, middle.text, last.text]);
        expect((await review(request, fixture.url)).comments.find((comment) => comment.body === 'Late target').target.attachments[0]).toMatchObject({ owner: { id: last.block_id }, quote: 'TARGET' });
        await user.page.keyboard.press('ControlOrMeta+Shift+z');
        await expect.poll(async () => (await textBlocks()).map((block) => block.text)).toEqual([expected]);
        await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
        await user.page.reload();
        await expect(body(user.page)).toContainText(expected);
        expect((await review(request, fixture.url)).comments.find((comment) => comment.body === 'Late target').target.attachments[0]).toMatchObject({ owner: { id: first.block_id }, quote: 'TARGET' });
        expect(user.errors).toEqual([]);
      } finally { await user.context.close(); }
    });
  }
}

for (const code of [false, true]) test(`a delayed multi-block deletion merges with agent edits and a second browser (${code ? 'code' : 'paragraphs'})`, async ({ browser, request }) => {
  const fixture = await createDocument(request, code ? '```\nKeep FIRST\nRemove middle\nLAST TARGET\n```' : 'Keep FIRST\n\nRemove middle\n\nLAST TARGET');
  const initial = (await blocks(request, fixture.url)).filter((block) => block.block_type !== 'code_block');
  const [first, , last] = initial;
  const writer = await openDocument(browser, fixture.path, 'Writer');
  const reader = await openDocument(browser, fixture.path, 'Reader');
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  let held = false;
  try {
    await writer.page.route('**/document-commands', async (route) => { held = true; await gate; await route.continue(); });
    await selectRange(writer.page, { id: first.block_id, offset: 5 }, { id: last.block_id, offset: 5 });
    await writer.page.keyboard.press('Backspace');
    await expect.poll(() => held).toBe(true);
    await transaction(request, fixture.url, [
      { op: 'comment.add', block_id: last.block_id, start: 5, end: 11, body: 'Concurrent target' },
      { op: 'replace_block_content', block_id: last.block_id, text: 'LAST TAR!GET' },
    ], first.document_clock);
    release();
    await expect(writer.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await expect.poll(async () => (await blocks(request, fixture.url)).filter((block) => block.block_type !== 'code_block').map((block) => block.text)).toEqual(['Keep TAR!GET']);
    await expect(body(writer.page)).toContainText('Keep TAR!GET');
    await expect(body(reader.page)).toContainText('Keep TAR!GET');
    const comment = (await review(request, fixture.url)).comments[0];
    expect(comment.target.attachments[0]).toMatchObject({ owner: { id: first.block_id }, quote: 'TAR!GET' });
    await body(writer.page).focus(); await writer.page.keyboard.press('ControlOrMeta+z');
    await expect.poll(async () => (await blocks(request, fixture.url)).filter((block) => block.block_type !== 'code_block').map((block) => block.text)).toEqual(['Keep FIRST', 'Remove middle', 'LAST TAR!GET']);
    expect((await review(request, fixture.url)).comments[0].target.attachments[0]).toMatchObject({ owner: { id: last.block_id }, quote: 'TAR!GET' });
    expect(writer.errors).toEqual([]); expect(reader.errors).toEqual([]);
  } finally { release(); await writer.context.close(); await reader.context.close(); }
});
