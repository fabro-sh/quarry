import { expect, test } from 'playwright/test';
import { blocks, body, createDocument, openDocument, paste, review, select, transaction, uiOrigin } from './helpers/native-document';

test.describe.configure({ timeout: 60000 });

test('an agent read keeps the original repeated-text comment target after browser typing', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'See TARGET here.');
  const [read] = await blocks(request, fixture.url);
  const user = await openDocument(browser, fixture.path, 'Writer');
  try {
    await select(user.page, read.block_id, 0);
    await user.page.keyboard.insertText('See TARGET ');
    await expect.poll(async () => (await blocks(request, fixture.url))[0].text).toBe('See TARGET See TARGET here.');
    const payload = { client_tx_id: crypto.randomUUID(), actor: { kind: 'agent' },
      ops: [{ op: 'comment.add', block_id: read.block_id, start: 4, end: 10, quote: 'TARGET', body: 'Original occurrence' }] };
    const unversioned = await request.post(`${fixture.url}/transactions`, { data: payload });
    expect(unversioned.status()).toBe(400);
    expect((await unversioned.json()).details.field).toBe('base_clock');
    expect((await review(request, fixture.url)).comments).toHaveLength(0);
    const versioned = { ...payload, base_clock: read.document_clock };
    const response = await request.post(`${fixture.url}/transactions`, { data: versioned });
    expect(response.ok(), await response.text()).toBe(true);
    const receipt = await response.json(); expect(receipt.status).toBe('committed_rebased');
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    const attachment = (await review(request, fixture.url)).comments[0].target.attachments[0];
    expect(attachment).toMatchObject({ start: 15, end: 21, quote: 'TARGET' });
    expect(await (await request.post(`${fixture.url}/transactions`, { data: versioned })).json()).toEqual(receipt);
    await user.page.reload();
    await expect(body(user.page)).toHaveText('See TARGET See TARGET here.');
    await expect(body(user.page).locator('[data-comment-id]')).toHaveText('TARGET');
    expect((await review(request, fixture.url)).comments[0].target.attachments[0]).toEqual(attachment);
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('a delayed agent deletion fails atomically and the browser can keep editing', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'See TARGET here.');
  const [read] = await blocks(request, fixture.url);
  const user = await openDocument(browser, fixture.path, 'Writer');
  try {
    await select(user.page, read.block_id, 0);
    await user.page.keyboard.insertText('Browser ');
    await expect.poll(async () => (await blocks(request, fixture.url))[0].text).toBe('Browser See TARGET here.');
    const response = await request.post(`${fixture.url}/transactions`, { data: {
      client_tx_id: crypto.randomUUID(), base_clock: read.document_clock, actor: { kind: 'agent' }, ops: [
        { op: 'comment.add', block_id: read.block_id, start: 4, end: 10, body: 'Never committed' },
        { op: 'delete_block', block_id: read.block_id },
      ],
    } });
    expect(response.status()).toBe(412);
    expect((await response.json()).code).toBe('PRECONDITION_FAILED');
    expect((await review(request, fixture.url)).comments).toHaveLength(0);
    await select(user.page, read.block_id, 24);
    await user.page.keyboard.type(' Still here.');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload();
    await expect(body(user.page)).toHaveText('Browser See TARGET here. Still here.');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('a private comment draft follows exact text through an agent rewrite and move and another browser split', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Before TARGET after.\n\nOther TARGET elsewhere.\n');
  const [first, duplicate] = await blocks(request, fixture.url);
  const writer = await openDocument(browser, fixture.path, 'Writer');
  const reviewer = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await select(reviewer.page, first.block_id, 7, 13);
    await reviewer.page.getByRole('button', { name: 'Comment', exact: true }).click();
    await reviewer.page.getByTestId('draft-input').fill('Keep the original target');
    await expect(writer.page.getByTestId('draft-composer')).toHaveCount(0);
    await transaction(request, fixture.url, [{ op: 'replace_block_content', block_id: first.block_id, text: 'Agent Before TARGET after.' }], first.document_clock);
    await expect(body(writer.page)).toContainText('Agent Before TARGET');
    await expect(body(reviewer.page)).toContainText('Agent Before TARGET');
    await select(writer.page, first.block_id, 16);
    await writer.page.keyboard.press('Enter');
    await expect.poll(async () => (await blocks(request, fixture.url)).length).toBe(3);
    const moved = (await blocks(request, fixture.url))[1];
    await transaction(request, fixture.url, [{ op: 'move_block', block_id: moved.block_id, position: 2 }], moved.document_clock);
    await expect.poll(async () => (await blocks(request, fixture.url))[1].block_id).toBe(duplicate.block_id);
    await reviewer.page.getByRole('button', { name: 'Submit comment', exact: true }).click();
    await expect.poll(async () => (await review(request, fixture.url)).comments.length).toBe(1);
    const comment = (await review(request, fixture.url)).comments[0];
    expect(comment.target.state).toBe('attached');
    expect(comment.target.attachments.map((part: { quote: string }) => part.quote).join('')).toBe('TARGET');
    expect(comment.target.attachments.every((part: { owner: { id: string } }) => part.owner.id !== duplicate.block_id)).toBe(true);
    for (const user of [writer, reviewer]) {
      await user.page.getByRole('tab', { name: /Comments/ }).click();
      await expect(user.page.getByText('Keep the original target', { exact: true })).toBeVisible();
      await expect.poll(() => body(user.page).locator(`[data-comment-id="${comment.id}"]`).allTextContents()).toEqual(['TAR', 'GET']);
      expect(user.errors).toEqual([]);
    }
  } finally { await writer.context.close(); await reviewer.context.close(); }
});

test('a lost command response retries the exact request without publishing a second version', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'TARGET\n');
  const [block] = await blocks(request, fixture.url);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    const before = await (await request.get(`${fixture.url}/versions`)).json();
    const ids: string[] = [];
    await user.page.route('**/document-commands', async (route) => {
      ids.push(route.request().postDataJSON().request_id);
      const response = await route.fetch();
      if (ids.length === 1) await route.abort('connectionreset');
      else await route.fulfill({ response });
    });
    await select(user.page, block.block_id, 0);
    await user.page.keyboard.insertText('Exactly once ');
    await expect.poll(() => ids.length).toBeGreaterThanOrEqual(2);
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    const after = await (await request.get(`${fixture.url}/versions`)).json();
    expect(after).toHaveLength(before.length + 1);
    expect(new Set(ids).size).toBe(1);
    expect((await blocks(request, fixture.url))[0].text).toBe('Exactly once TARGET');
    await user.page.reload();
    await expect(body(user.page)).toHaveText('Exactly once TARGET');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('browser undo and redo preserve a comment delivered after the edit', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'TARGET\n');
  const [block] = await blocks(request, fixture.url);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await select(user.page, block.block_id, 0);
    await user.page.keyboard.insertText('Prefix ');
    await expect.poll(async () => (await blocks(request, fixture.url))[0].text).toBe('Prefix TARGET');
    const [afterEdit] = await blocks(request, fixture.url);
    await transaction(request, fixture.url, [{ op: 'comment.add', block_id: afterEdit.block_id, start: 7, end: 13, body: 'After the edit' }], afterEdit.document_clock);
    const comment = (await review(request, fixture.url)).comments[0];
    await expect(body(user.page).locator(`[data-comment-id="${comment.id}"]`)).toHaveText('TARGET');
    await body(user.page).focus();
    await user.page.keyboard.press('ControlOrMeta+z');
    await expect.poll(async () => (await blocks(request, fixture.url))[0].text).toBe('TARGET');
    await expect(body(user.page).locator(`[data-comment-id="${comment.id}"]`)).toHaveText('TARGET');
    await user.page.keyboard.press('ControlOrMeta+Shift+z');
    await expect.poll(async () => (await blocks(request, fixture.url))[0].text).toBe('Prefix TARGET');
    expect((await review(request, fixture.url)).comments[0].target.attachments[0].quote).toBe('TARGET');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('an offline browser draft survives page closure and merges with an agent edit on reopening', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'TARGET\n');
  const [block] = await blocks(request, fixture.url);
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await user.context.setOffline(true);
    await select(user.page, block.block_id, 0);
    await user.page.keyboard.insertText('Offline ');
    await expect(body(user.page)).toHaveText('Offline TARGET');
    await expect(user.page.getByRole('button', { name: 'Download draft', exact: true })).toBeVisible();
    await user.page.close();
    await transaction(request, fixture.url, [{ op: 'replace_block_content', block_id: block.block_id, text: 'TARGET agent' }], block.document_clock);
    await user.context.setOffline(false);
    const reopened = await user.context.newPage();
    reopened.on('pageerror', (error) => user.errors.push(error.message));
    await reopened.goto(`${uiOrigin}${fixture.path}`);
    await expect(body(reopened)).toHaveText('Offline TARGET agent');
    await expect.poll(async () => (await blocks(request, fixture.url))[0].text).toBe('Offline TARGET agent');
    await expect(reopened.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('a comment on proposed Unicode text follows browser acceptance and survives reload', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Before TARGET after.\n');
  const [block] = await blocks(request, fixture.url);
  await transaction(request, fixture.url, [{ op: 'suggestion.add', block_id: block.block_id, start: 7, end: 13, replacement: '😀proposed', body: 'Clearer wording' }], block.document_clock);
  const proposal = (await review(request, fixture.url)).suggestions[0];
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await user.page.getByRole('tab', { name: /Comments/ }).click();
    await select(user.page, proposal.id, 0, 10);
    await user.page.getByRole('button', { name: 'Comment', exact: true }).click();
    await user.page.getByTestId('draft-input').fill('Keep this proposal');
    await user.page.getByRole('button', { name: 'Submit comment', exact: true }).click();
    await expect.poll(async () => (await review(request, fixture.url)).comments.length).toBe(1);
    await user.page.getByTestId('suggestion-card').getByRole('button', { name: 'Accept suggestion', exact: true }).click();
    await expect(body(user.page)).toHaveText('Before 😀proposed after.');
    await expect.poll(async () => (await review(request, fixture.url)).suggestions[0].status).toBe('resolved');
    await user.page.reload();
    await expect(body(user.page)).toHaveText('Before 😀proposed after.');
    const comment = (await review(request, fixture.url)).comments[0];
    expect(comment.target.attachments[0]).toMatchObject({ owner: { kind: 'block', id: block.block_id }, quote: '😀proposed' });
    await expect(body(user.page).locator(`[data-comment-id="${comment.id}"]`)).toHaveText('😀proposed');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('review Markdown imports through HTTP and a downloaded archive opens as a separate document with intact targets', async ({ browser, request }) => {
  const fixture = await createDocument(request, '---\ntitle: Portable review\n---\nSee {==TARGET==}{>>Keep this<<}{#c} and {~~old~>new~~}{#s}.\n');
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await user.page.getByRole('tab', { name: /Comments/ }).click();
    await expect(user.page.getByText('Keep this', { exact: true })).toBeVisible();
    const download = user.page.waitForEvent('download');
    await user.page.getByRole('region', { name: 'Editor', exact: true }).getByRole('button', { name: 'Document actions', exact: true }).click();
    await user.page.getByRole('menuitem', { name: 'Download review archive', exact: true }).click();
    const archive = await download;
    await user.page.getByLabel('Import review archive', { exact: true }).setInputFiles((await archive.path())!);
    const link = user.page.getByRole('link', { name: 'Open imported document', exact: true });
    await expect(link).toBeVisible(); await link.click();
    await expect(body(user.page)).toContainText('See TARGET and newold.');
    await user.page.getByRole('tab', { name: /Comments/ }).click();
    await expect(user.page.getByText('Keep this', { exact: true })).toBeVisible();
    await user.page.getByTestId('suggestion-card').getByRole('button', { name: 'Accept suggestion', exact: true }).click();
    await expect(body(user.page)).toContainText('See TARGET and new.');
    const original = await review(request, fixture.url);
    expect(original.suggestions[0].status).toBe('open');
    const archiveResponse = await request.get(`${fixture.url}/archive`);
    expect(archiveResponse.ok()).toBe(true);
    expect((await archiveResponse.json()).metadata.title).toBe('Portable review');
    expect((await request.post(`${fixture.url}/archive`, { data: await archiveResponse.json() })).status()).toBe(412);
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

for (const offset of [0, 7]) test(`browser rich paste at offset ${offset} and table controls preserve the following comment`, async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Before TARGET after.\n'); const [block] = await blocks(request, fixture.url);
  await transaction(request, fixture.url, [{ op: 'comment.add', block_id: block.block_id, start: 7, end: 13, body: 'Surviving text' }], block.document_clock);
  const user = await openDocument(browser, fixture.path, 'Writer');
  try {
    await select(user.page, block.block_id, offset);
    await paste(user.page, '<h2>Pasted heading</h2><table><tr><th>Header</th></tr><tr><td><strong>Cell</strong></td></tr></table>', 'Pasted heading\nHeader\nCell');
    await expect(body(user.page).locator(offset === 0 ? 'h2' : '.slate-p').first()).toHaveText(offset === 0 ? 'Pasted heading' : 'Before Pasted heading');
    await expect(body(user.page).locator('td strong')).toHaveText('Cell');
    await expect.poll(async () => (await blocks(request, fixture.url)).filter((block) => block.block_type === 'table').length).toBe(1);
    const cell = (await blocks(request, fixture.url)).find((block) => block.text === 'Cell')!;
    await select(user.page, cell.block_id, 1);
    await body(user.page).getByRole('button', { name: 'Add row', exact: true }).click();
    await body(user.page).getByRole('button', { name: 'Add column', exact: true }).click();
    await expect(body(user.page).locator('tr')).toHaveCount(3);
    await expect(body(user.page).locator('tr').first().locator('th')).toHaveCount(2);
    expect((await review(request, fixture.url)).comments[0].target.attachments[0].quote).toBe('TARGET');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload(); await expect(body(user.page).locator('tr')).toHaveCount(3);
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('Mermaid rendering and explicit source edits use native block attributes', async ({ browser, request }) => {
  const fixture = await createDocument(request, '```mermaid\nflowchart LR\n  A --> B\n```\n\nSee [[Another document]] here.\n');
  const user = await openDocument(browser, fixture.path, 'Writer');
  try {
    await expect(user.page.getByTestId('mermaid-diagram').locator('svg')).toBeVisible();
    await user.page.getByRole('button', { name: 'Edit Mermaid source', exact: true }).click();
    await user.page.getByRole('textbox', { name: 'Mermaid source', exact: true }).fill('flowchart LR\n  A --> C');
    await user.page.getByRole('button', { name: 'Save source', exact: true }).click();
    await expect(user.page.getByTestId('mermaid-diagram').locator('svg')).toContainText('C');
    await expect(body(user.page).locator('.slate-wikilink')).toContainText('Another document');
    const paragraph = (await blocks(request, fixture.url)).find((block) => block.text.includes('[[Another document]]'))!;
    await select(user.page, paragraph.block_id, 0); await user.page.keyboard.type('Updated ');
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload();
    await expect(user.page.getByTestId('mermaid-diagram').locator('svg')).toContainText('C');
    await expect(body(user.page).locator('.slate-wikilink')).toContainText('Another document');
    expect((await blocks(request, fixture.url)).find((block) => block.block_id === paragraph.block_id)!.text).toBe('Updated See [[Another document]] here.');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('remote cursors follow native positions through agent insertion without publishing document versions', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'Before TARGET after.\n'); const [block] = await blocks(request, fixture.url);
  const first = await openDocument(browser, fixture.path, 'First reviewer');
  const second = await openDocument(browser, fixture.path, 'Second reviewer');
  try {
    const before = (await (await request.get(`${fixture.url}/versions`)).json()).length;
    await select(first.page, block.block_id, 10);
    await select(second.page, block.block_id, 13);
    await expect(first.page.locator('.quarry-remote-caret')).toContainText('Second reviewer');
    await expect(second.page.locator('.quarry-remote-caret')).toContainText('First reviewer');
    expect((await (await request.get(`${fixture.url}/versions`)).json()).length).toBe(before);
    await transaction(request, fixture.url, [{ op: 'replace_block_content', block_id: block.block_id, text: 'Agent Before TARGET after.' }], block.document_clock);
    await expect(body(first.page)).toHaveText('Agent Before TARGET after.');
    await expect(body(second.page)).toHaveText('Agent Before TARGET after.');
    await expect.poll(() => second.page.evaluate((id) => {
      const block = document.querySelector(`[data-block-id="${id}"]`)!;
      const leaf = block.querySelector('[data-slate-string]')!.firstChild!;
      const range = document.createRange(); range.setStart(leaf, 16); range.collapse(true);
      const target = range.getBoundingClientRect();
      const cursor = document.querySelector('.quarry-remote-caret')!.getBoundingClientRect();
      return Math.abs(cursor.left - target.left) < 2 && Math.abs(cursor.top - target.top) < 2;
    }, block.block_id)).toBe(true);
    expect(first.errors).toEqual([]); expect(second.errors).toEqual([]);
  } finally { await first.context.close(); await second.context.close(); }
});

test('review replies, resolve, reopen and delete persist through native state and reload', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'See {==TARGET==}{>>Thread body<<}{#c}.\n\n---\ncomments:\n  c: {by: Reviewer}\n');
  const user = await openDocument(browser, fixture.path, 'Reviewer');
  try {
    await user.page.getByRole('tab', { name: /Comments/ }).click();
    const thread = user.page.getByLabel('Comment by Reviewer', { exact: true });
    await thread.click();
    await thread.getByRole('textbox', { name: 'Reply', exact: true }).fill('Reply body');
    await thread.getByRole('button', { name: 'Submit reply', exact: true }).click();
    await expect(thread.getByText('Reply body', { exact: true })).toBeVisible();
    await thread.getByRole('button', { name: 'Resolve comment', exact: true }).click();
    await expect(thread).toHaveCount(0); await expect(body(user.page).locator('[data-comment-id="c"]')).toHaveCount(0);
    await user.page.getByLabel('Show resolved comments and closed suggestions', { exact: true }).check();
    await expect(thread.getByText('Reply body', { exact: true })).toBeVisible();
    await thread.getByRole('button', { name: 'Reopen comment', exact: true }).click();
    await expect(body(user.page).locator('[data-comment-id="c"]')).toHaveText('TARGET');
    await thread.getByRole('button', { name: 'Comment actions', exact: true }).click();
    await user.page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
    await expect(body(user.page).locator('[data-comment-id="c"]')).toHaveCount(0);
    await expect(user.page.getByLabel('Save status', { exact: true })).toHaveText('Saved');
    await user.page.reload(); await expect(body(user.page)).toHaveText('See TARGET.');
    await expect(body(user.page).locator('[data-comment-id="c"]')).toHaveCount(0);
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});

test('viewer invitations disable document and review mutations while receiving edits', async ({ browser, request }) => {
  const fixture = await createDocument(request, 'TARGET\n'); const [block] = await blocks(request, fixture.url);
  const response = await request.post(`${fixture.url}/share`, { data: { role: 'viewer' } }); expect(response.ok()).toBe(true);
  const invite = await response.json(); const user = await openDocument(browser, `${fixture.path}?token=${encodeURIComponent(invite.id)}`, 'Viewer');
  try {
    await expect(body(user.page)).toHaveAttribute('contenteditable', 'false');
    await expect(user.page.getByRole('button', { name: 'Comment', exact: true })).toHaveCount(0);
    await body(user.page).focus(); await user.page.keyboard.type('Must not edit');
    expect((await blocks(request, fixture.url))[0].text).toBe('TARGET');
    await transaction(request, fixture.url, [{ op: 'replace_block_content', block_id: block.block_id, text: 'Agent TARGET' }], block.document_clock);
    await expect(body(user.page)).toHaveText('Agent TARGET');
    await user.page.reload(); await expect(body(user.page)).toHaveAttribute('contenteditable', 'false');
    expect(user.errors).toEqual([]);
  } finally { await user.context.close(); }
});
