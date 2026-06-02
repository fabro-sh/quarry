import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page, type Route } from 'playwright/test';

interface MockDocument {
  byteSize?: number;
  content: string;
  contentHash?: string | null;
  contentType?: string;
  id: string;
  metadata?: Record<string, unknown>;
  path: string;
  version: string;
}

interface MockLibrary {
  id: string;
  slug: string;
}

interface MockConflict {
  conflict_path?: string | null;
  id: string;
  ours_version_id?: string | null;
  path: string;
  theirs_version_id?: string | null;
}

interface MockLink {
  alias?: string | null;
  end_offset?: number;
  resolution_status?: 'resolved' | 'unresolved' | 'ambiguous';
  resolved?: boolean;
  src_doc_id?: string;
  src_path?: string;
  src_version_id?: string;
  start_offset?: number;
  target_anchor?: string | null;
  target_doc_id?: string | null;
  target_kind?: string;
  target_path?: string | null;
  target_text?: string;
}

interface MockVersion {
  content: string;
  created_at?: string;
  id: string;
}

test.describe('Quarry Browser smoke flows', () => {
  test.beforeEach(async ({ page }) => {
    await disableEventSource(page);
  });


  test('creates, edits, saves, and reloads a markdown document', async ({ page }) => {
    const api = await installMockApi(page, {
      documents: [
        {
          content: '# Daily\n',
          id: 'doc-daily',
          metadata: { title: 'Daily' },
          path: 'daily.md',
          version: 'v1',
        },
      ],
    });

    await page.goto('/');

    await expect(page.getByRole('treeitem', { name: /Daily/ })).toBeVisible();
    page.once('dialog', async (dialog) => {
      expect(dialog.message()).toBe('New document path');
      await dialog.accept('new.md');
    });
    await page.getByRole('button', { name: 'Create document' }).click();

    await expect(page.getByRole('treeitem', { name: /new\.md/ })).toBeVisible();
    await expect(page.getByLabel('Plate markdown editor')).toContainText('Untitled');
    expect(api.createHeaders).toContain('*');

    // Autosave persists the edit a beat after typing — there is no Save button.
    const editor = page.getByLabel('Plate markdown editor');
    await editor.click();
    await page.keyboard.press('End');
    await page.keyboard.type(' edited');
    await expect(page.locator('[aria-label="Save status"]')).toContainText('Saved');
    expect(api.saveHeaders).toContain('"v-new"');

    await page.reload();

    await expect(page.getByRole('treeitem', { name: /new\.md/ })).toBeVisible();
    await page.getByRole('treeitem', { name: /new\.md/ }).click();
    await expect(page.getByLabel('Plate markdown editor')).toContainText('edited');
  });

  test('inserts and removes a hyperlink, persisting it as markdown', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        { content: 'Visit example soon.\n', id: 'doc-link', metadata: { title: 'Linky' }, path: 'link.md', version: 'v1' },
      ],
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Linky/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('Visit example soon');

    // Select "example", open the insert popover from the floating toolbar, and
    // enter a URL.
    await page.getByText('example', { exact: false }).dblclick();
    await page.getByRole('button', { name: 'Link', exact: true }).click();
    const urlInput = page.getByPlaceholder('Paste link');
    await urlInput.fill('https://example.com');
    await urlInput.press('Enter');

    // The word becomes a real anchor pointing at the URL.
    const link = editor.locator('a[href*="example.com"]');
    await expect(link).toHaveText('example');

    // Autosave persists it; reloading round-trips the markdown link back into an
    // anchor.
    await expect(page.locator('[aria-label="Save status"]')).toContainText('Saved');
    await page.reload();
    await page.getByRole('treeitem', { name: /Linky/ }).click();
    await expect(editor.locator('a[href*="example.com"]')).toHaveText('example');

    // Putting the cursor in the link reveals the edit popover; Remove unlinks it.
    await editor.locator('a[href*="example.com"]').click();
    await page.getByRole('button', { name: 'Remove link' }).click();
    await expect(editor.locator('a[href*="example.com"]')).toHaveCount(0);
    await expect(editor).toContainText('example');
  });

  test('renders a wiki-link chip and navigates to its target on click', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        { content: 'See [[Guide]] for more.\n', id: 'doc-notes', metadata: { title: 'Notes' }, path: 'notes.md', version: 'v1' },
        { content: '# Guide\n\nThe guide body.\n', id: 'doc-guide', metadata: { title: 'Guide' }, path: 'guide.md', version: 'v-guide' },
      ],
      links: {
        'notes.md': {
          outgoing: [link({ target_text: 'Guide', target_path: 'guide.md', target_doc_id: 'doc-guide' })],
        },
      },
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Notes/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('See');

    const chip = editor.getByTestId('wikilink');
    await expect(chip).toHaveText('Guide');
    await expect(chip).toHaveAttribute('data-resolved', 'true');

    await chip.click();
    await expect(editor).toContainText('The guide body.');
  });

  test('converts a typed [[..]] into a wiki-link and round-trips it', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        { content: 'Start.\n', id: 'doc-wt', metadata: { title: 'Wikitype' }, path: 'wt.md', version: 'v1' },
      ],
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Wikitype/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('Start.');

    await editor.click();
    await page.keyboard.press('End');
    await page.keyboard.type(' [[Note]] end');

    // The completed [[Note]] becomes a chip, and typing continues after it.
    await expect(editor.getByTestId('wikilink')).toHaveText('Note');
    await expect(editor).toContainText('end');

    // Autosave persists it; reloading round-trips `[[Note]]` back into a chip
    // (which only happens if it wasn't escaped to `\[\[Note]]`).
    await expect(page.locator('[aria-label="Save status"]')).toContainText('Saved');
    await page.reload();
    await page.getByRole('treeitem', { name: /Wikitype/ }).click();
    await expect(editor.getByTestId('wikilink')).toHaveText('Note');
  });

  test('drops an image, stores it as an asset, and references it as markdown', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        { content: 'Drop here.\n', id: 'doc-img', metadata: { title: 'Imgdoc' }, path: 'imgdoc.md', version: 'v1' },
      ],
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Imgdoc/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('Drop here.');
    await editor.click();

    // Drop a 1x1 PNG onto the editor (a real File in a DataTransfer, with drop
    // coordinates so Plate can resolve the caret location).
    const dataTransfer = await page.evaluateHandle(() => {
      const dt = new DataTransfer();
      const bytes = Uint8Array.from(
        atob('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=='),
        (c) => c.charCodeAt(0)
      );
      const file = new File([bytes], 'pic.png', { type: 'image/png' });
      dt.items.add(file);
      return dt;
    });
    const box = (await editor.boundingBox())!;
    await editor.dispatchEvent('drop', { dataTransfer, clientX: box.x + 40, clientY: box.y + 20 });

    // The upload PUTs the bytes to assets/<hash>.png and the image renders from
    // the serve endpoint.
    const img = editor.locator('img');
    await expect(img).toHaveAttribute('src', /\/v1\/libraries\/notes\/documents\/assets\/[0-9a-f]+\.png/);

    // Autosave persists `![](assets/<hash>.png)`; reloading round-trips it.
    await expect(page.locator('[aria-label="Save status"]')).toContainText('Saved');
    await page.reload();
    await page.getByRole('treeitem', { name: /Imgdoc/ }).click();
    await expect(editor.locator('img')).toHaveAttribute('src', /assets\/[0-9a-f]+\.png/);
  });

  test('renders a mermaid code block as a diagram with a Code/Preview toggle', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: '# Flow\n\n```mermaid\ngraph TD\n  A --> B\n```\n',
          id: 'doc-mmd',
          metadata: { title: 'Mermaiddoc' },
          path: 'mmd.md',
          version: 'v1',
        },
      ],
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Mermaiddoc/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('Flow');

    const diagram = editor.getByTestId('mermaid-diagram');
    const toggle = editor.getByTestId('mermaid-toggle');

    // Default Preview: the diagram renders as an SVG.
    await expect(diagram.locator('svg')).toBeVisible({ timeout: 20000 });
    await expect(toggle).toContainText('Code');

    // Toggle to Code: the source textarea shows, the diagram is gone.
    await toggle.click();
    const source = editor.getByTestId('mermaid-source');
    await expect(source).toBeVisible();
    await expect(source).toHaveValue(/graph TD/);
    await expect(diagram).toHaveCount(0);
    await expect(toggle).toContainText('Preview');

    // Toggle back to Preview: the diagram renders again.
    await toggle.click();
    await expect(editor.getByTestId('mermaid-diagram').locator('svg')).toBeVisible({ timeout: 20000 });
  });

  test('turns a block into a mermaid diagram from the toolbar', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        { content: '# Make\n\ngraph TD; A-->B\n', id: 'doc-mk', metadata: { title: 'Makemmd' }, path: 'makemmd.md', version: 'v1' },
      ],
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Makemmd/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('graph TD');

    // Select the line (valid mermaid) and convert it via Turn into → Mermaid.
    await page.getByText('graph TD', { exact: false }).click({ clickCount: 3 });
    await page.getByRole('button', { name: 'Turn into' }).click();
    await page.getByRole('menuitem', { name: 'Mermaid' }).click();

    // It renders as a diagram (content present → opens in Preview).
    await expect(editor.getByTestId('mermaid-diagram').locator('svg')).toBeVisible({ timeout: 20000 });
  });

  test('deletes a mermaid diagram as one block on backspace from the next line', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: '# T\n\n```mermaid\ngraph TD\n  A --> B\n```\n\nAfter line.\n',
          id: 'doc-bd',
          metadata: { title: 'Bdel' },
          path: 'bdel.md',
          version: 'v1',
        },
      ],
    });
    await page.goto('/');
    await page.getByRole('treeitem', { name: /Bdel/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor.getByTestId('mermaid-diagram').locator('svg')).toBeVisible({ timeout: 20000 });

    // Select the line after the diagram (a real range Slate maps), collapse to
    // its start (ArrowLeft), then backspace.
    await editor.getByText('After line').selectText();
    await expect(editor.getByText('After line')).toBeVisible();
    await page.keyboard.press('ArrowLeft');
    await page.keyboard.press('Backspace');

    // The whole diagram block is removed; the source is never edited (no error),
    // and the following line survives.
    await expect(editor.getByTestId('mermaid-error')).toHaveCount(0);
    await expect(editor.getByTestId('mermaid-diagram')).toHaveCount(0);
    await expect(editor.getByText('After line')).toBeVisible();
  });

  test('can type a new block after a trailing mermaid diagram', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        { content: '# Title\n\n```mermaid\ngraph TD; A-->B\n```\n', id: 'doc-tr', metadata: { title: 'Trailmmd' }, path: 'trailmmd.md', version: 'v1' },
      ],
    });
    await page.goto('/');
    await page.getByRole('treeitem', { name: /Trailmmd/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor.getByTestId('mermaid-diagram').locator('svg')).toBeVisible({ timeout: 20000 });

    // The diagram is not the last block — there's a visible editable line after
    // it to click into and type.
    const lastIsDiagram = await editor.evaluate(
      (el) => !!el.lastElementChild?.querySelector('[data-testid="mermaid-diagram"]')
    );
    expect(lastIsDiagram).toBe(false);

    const box = (await editor.boundingBox())!;
    await editor.click({ position: { x: 40, y: box.height - 12 } });
    await page.keyboard.type('a new paragraph');
    await expect(editor).toContainText('a new paragraph');
  });

  test('renders a GFM table with editable cells', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        { content: '# Doc\n\n| Name | Role |\n| --- | --- |\n| Ana | Lead |\n', id: 'doc-tbl', metadata: { title: 'Tbl' }, path: 'tbl.md', version: 'v1' },
      ],
    });
    await page.goto('/');
    await page.getByRole('treeitem', { name: /Tbl/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor.locator('table')).toBeVisible();
    await expect(editor.locator('th', { hasText: 'Name' })).toBeVisible();
    await expect(editor.locator('td', { hasText: 'Lead' })).toBeVisible();
  });

  test('turns a block into a table from the toolbar', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        { content: '# Doc\n\nseed line\n', id: 'doc-t2', metadata: { title: 'T2' }, path: 't2.md', version: 'v1' },
      ],
    });
    await page.goto('/');
    await page.getByRole('treeitem', { name: /T2/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await editor.getByText('seed line').selectText();
    await page.getByRole('button', { name: 'Turn into' }).click();
    await page.getByRole('menuitem', { name: 'Table' }).click();
    await expect(editor.locator('th')).toHaveCount(3);
    await expect(editor.locator('td')).toHaveCount(6);
  });

  test('typing in a table cell autosaves', async ({ page }) => {
    const api = await installMockApi(page, {
      documents: [
        { content: '# Doc\n\n| A | B |\n| --- | --- |\n| x | y |\n', id: 'doc-t3', metadata: { title: 'T3' }, path: 't3.md', version: 'v1' },
      ],
    });
    await page.goto('/');
    await page.getByRole('treeitem', { name: /T3/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await editor.locator('td', { hasText: 'x' }).click();
    await page.keyboard.type('X1');
    await expect.poll(() => api.saveHeaders.length).toBeGreaterThan(0);
  });

  test('setting a column alignment persists to markdown', async ({ page }) => {
    const api = await installMockApi(page, {
      documents: [
        { content: '# Doc\n\n| A | B |\n| --- | --- |\n| x | y |\n', id: 'doc-t4', metadata: { title: 'T4' }, path: 't4.md', version: 'v1' },
      ],
    });
    await page.goto('/');
    await page.getByRole('treeitem', { name: /T4/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await editor.locator('th', { hasText: 'A' }).hover();
    await editor.getByRole('button', { name: 'Column options' }).first().click();
    await page.getByRole('menuitem', { name: 'Align center' }).click();
    // The reactive align read must restyle the cell, not just the saved markdown.
    await expect(editor.locator('th', { hasText: 'A' })).toHaveClass(/text-center/);
    await expect.poll(() => api.lastSavedBody('t4.md')).toContain(':-:');
  });

  test('column alignment follows its column when a column is inserted', async ({ page }) => {
    const api = await installMockApi(page, {
      documents: [
        { content: '# Doc\n\n| A | B |\n| --- | --- |\n| x | y |\n', id: 'doc-t4b', metadata: { title: 'T4b' }, path: 't4b.md', version: 'v1' },
      ],
    });
    await page.goto('/');
    await page.getByRole('treeitem', { name: /T4b/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    // Center-align column A.
    await editor.locator('th', { hasText: 'A' }).hover();
    await editor.getByRole('button', { name: 'Column options' }).first().click();
    await page.getByRole('menuitem', { name: 'Align center' }).click();
    await expect.poll(() => api.lastSavedBody('t4b.md')).toContain(':-:');
    // Insert a column to the LEFT of A; the centered column must move to slot 2,
    // so a plain delimiter now precedes the centered one.
    await editor.locator('th', { hasText: 'A' }).hover();
    await editor.getByRole('button', { name: 'Column options' }).first().click();
    await page.getByRole('menuitem', { name: 'Insert column left' }).click();
    // GFM serializes the empty column's delimiter minimally (`-`), so a plain
    // delimiter now precedes the centered one — the center moved off column 0.
    await expect.poll(() => api.lastSavedBody('t4b.md')).toMatch(/-+\s*\|\s*:-:/);
  });

  test('deletes a table column', async ({ page }) => {
    const api = await installMockApi(page, {
      documents: [
        { content: '# Doc\n\n| A | B | C |\n| --- | --- | --- |\n| x | y | z |\n', id: 'doc-t6', metadata: { title: 'T6' }, path: 't6.md', version: 'v1' },
      ],
    });
    await page.goto('/');
    await page.getByRole('treeitem', { name: /T6/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor.locator('th')).toHaveCount(3);
    const bHeader = editor.locator('th', { hasText: 'B' });
    await bHeader.hover();
    await bHeader.getByRole('button', { name: 'Column options' }).click();
    await page.getByRole('menuitem', { name: 'Delete column' }).click();
    await expect(editor.locator('th')).toHaveCount(2);
    await expect(editor.locator('th', { hasText: 'B' })).toHaveCount(0);
    // Saved markdown reflects the deletion (poll past the empty-initial-body).
    await expect.poll(() => api.lastSavedBody('t6.md')).toContain('C');
    expect(api.lastSavedBody('t6.md')).not.toContain('B');
  });

  test('deletes a table row', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        { content: '# Doc\n\n| A | B |\n| --- | --- |\n| x | y |\n| p | q |\n', id: 'doc-t7', metadata: { title: 'T7' }, path: 't7.md', version: 'v1' },
      ],
    });
    await page.goto('/');
    await page.getByRole('treeitem', { name: /T7/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor.locator('tr')).toHaveCount(3);
    const aHeader = editor.locator('th', { hasText: 'A' });
    await aHeader.hover();
    await aHeader.getByRole('button', { name: 'Column options' }).click();
    await page.getByRole('menuitem', { name: 'Delete row' }).click();
    await expect(editor.locator('tr')).toHaveCount(2);
    await expect(editor.locator('th')).toHaveCount(2); // new header row promoted (x, y)
  });

  test('opens the browser and selects a library', async ({ page }) => {
    await installMockApi(page, {
      documents: [],
      documentsByLibrary: {
        personal: [
          {
            content: '# Personal\n',
            id: 'doc-personal',
            metadata: { title: 'Personal' },
            path: 'personal.md',
            version: 'v-personal',
          },
        ],
        work: [
          {
            content: '# Work\n',
            id: 'doc-work',
            metadata: { title: 'Work' },
            path: 'work.md',
            version: 'v-work',
          },
        ],
      },
      libraries: [
        { id: 'lib-personal', slug: 'personal' },
        { id: 'lib-work', slug: 'work' },
      ],
    });

    await page.goto('/');

    const switcher = page.getByRole('combobox', { name: 'Library switcher' });
    await expect(switcher).toHaveValue('personal');
    await switcher.selectOption('work');

    await expect(page).toHaveURL(/\/lib\/work$/);
    await expect(page.getByRole('treeitem', { name: /Work/ })).toBeVisible();
    await expect(page.getByRole('treeitem', { name: /Personal/ })).not.toBeVisible();

    await page.getByRole('treeitem', { name: /Work/ }).click();
    await expect(page.getByLabel('Plate markdown editor')).toContainText('Work');
    await expect(page.evaluate(() => localStorage.getItem('quarry:active-library'))).resolves.toBe('work');
  });

  test('supports the main workflow without pointer input', async ({ page }) => {
    await installMockApi(page, {
      documents: [],
      documentsByLibrary: {
        personal: [],
        work: [
          {
            content: '# Daily\n',
            id: 'doc-daily',
            metadata: { title: 'Daily' },
            path: 'daily.md',
            version: 'v-daily',
          },
          {
            content: '# Guide\n',
            id: 'doc-guide',
            metadata: { title: 'Guide' },
            path: 'guide.md',
            version: 'v-guide',
          },
        ],
      },
      libraries: [
        { id: 'lib-personal', slug: 'personal' },
        { id: 'lib-work', slug: 'work' },
      ],
    });

    await page.goto('/');

    const switcher = page.getByRole('combobox', { name: 'Library switcher' });
    await expect(switcher.locator('option')).toHaveCount(2);
    await switcher.focus();
    await page.keyboard.press('ArrowDown');
    await page.keyboard.press('Enter');

    await expect(switcher).toHaveValue('work');
    await expect(page).toHaveURL(/\/lib\/work$/);

    await page.getByRole('treeitem', { name: /Daily/ }).focus();
    await page.keyboard.press('Enter');
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('Daily');

    await page.getByRole('button', { name: 'Search' }).focus();
    await page.keyboard.press('Enter');
    const search = page.getByRole('textbox', { name: 'Search' });
    await expect(search).toBeFocused();
    await page.keyboard.type('guide');
    const results = page.getByRole('listbox', { name: 'Search results' });
    await expect(results.getByRole('option', { name: /Guide/ })).toBeVisible();
    await results.focus();
    await page.keyboard.press('Enter');
    await expect(editor).toContainText('Guide');

    await page.keyboard.press('ControlOrMeta+K');
    const palette = page.getByRole('dialog', { name: 'Command palette' });
    await expect(palette).toBeVisible();
    await page.keyboard.type('daily');
    await page.keyboard.press('Enter');
    await expect(editor).toContainText('Daily');
  });

  test('runs create, move, search, sync, settings, and delete from the command palette', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: '# Guide\n',
          id: 'doc-guide',
          metadata: { title: 'Guide' },
          path: 'guide.md',
          version: 'v-guide',
        },
      ],
    });

    await page.goto('/');

    page.once('dialog', async (dialog) => {
      expect(dialog.message()).toBe('New document path');
      await dialog.accept('palette-new.md');
    });
    await runCommand(page, 'create', 'Create document');
    await expect(page.getByRole('treeitem', { name: /palette-new\.md/ })).toBeVisible();
    await expect(page.getByLabel('Plate markdown editor')).toContainText('Untitled');

    page.once('dialog', async (dialog) => {
      expect(dialog.message()).toBe('Move document to path');
      expect(dialog.defaultValue()).toBe('palette-new.md');
      await dialog.accept('palette-moved.md');
    });
    await runCommand(page, 'move', 'Move current document');
    await expect(page.getByRole('treeitem', { name: /palette-moved\.md/ })).toBeVisible();
    await expect(page).toHaveURL(/\/lib\/notes\/documents\/palette-moved\.md$/);

    await runCommand(page, 'guide', 'Search server for "guide"');
    await expect(page.getByRole('listbox', { name: 'Search results' }).getByRole('option', { name: /Guide/ })).toBeVisible();

    await runCommand(page, 'sync', 'Sync with Git peer');
    await expect(page.getByRole('dialog', { name: 'Git operations' })).toBeVisible();
    await page.getByRole('button', { name: 'Close' }).click();

    await runCommand(page, 'settings', 'Open settings');
    await expect(page.getByRole('dialog', { name: 'Workspace settings' })).toBeVisible();
    await page.getByRole('button', { name: 'Close settings' }).click();

    page.once('dialog', async (dialog) => {
      expect(dialog.message()).toBe('Delete palette-moved.md?');
      await dialog.accept();
    });
    await runCommand(page, 'delete', 'Delete current document');
    await expect(page.getByRole('treeitem', { name: /palette-moved\.md/ })).not.toBeVisible();
    await expect(page.getByText('No document open')).toBeVisible();
  });

  test('passes automated accessibility checks for shell and conflict workflows', async ({ page }) => {
    await installMockApi(page, {
      conflicts: [
        {
          conflict_path: 'conflict.sibling.md',
          id: 'conflict-a11y',
          ours_version_id: 'ours',
          path: 'conflict.md',
          theirs_version_id: 'theirs',
        },
      ],
      documents: [
        {
          content: '# Conflict\n',
          id: 'doc-conflict',
          metadata: { title: 'Conflict' },
          path: 'conflict.md',
          version: 'head',
        },
        {
          content: '# Theirs\n',
          id: 'doc-conflict-sibling',
          metadata: { title: 'Conflict sibling' },
          path: 'conflict.sibling.md',
          version: 'theirs',
        },
      ],
      versions: {
        'conflict.md': [{ id: 'ours', content: '# Ours\n' }],
        'conflict.sibling.md': [{ id: 'theirs', content: '# Theirs\n' }],
      },
    });

    await page.goto('/');
    await expect(page.locator('[data-tree-path="conflict.md"]')).toBeVisible();
    await expectNoAxeViolations(page, 'workspace shell');

    await page.keyboard.press('ControlOrMeta+K');
    await expect(page.getByRole('dialog', { name: 'Command palette' })).toBeVisible();
    await expectNoAxeViolations(page, 'command palette');
    await page.keyboard.press('Escape');

    await page.getByRole('tab', { name: 'Conflicts' }).click();
    await expect(page.getByText('conflict.md open')).toBeVisible();
    await expectNoAxeViolations(page, 'conflicts tab');

    await page.getByLabel('Open conflict conflict-a11y').click();
    await expect(page.getByRole('dialog', { name: 'Resolve conflict' })).toBeVisible();
    await expectNoAxeViolations(page, 'conflict resolution dialog');
  });

  test('shows a stale-save workflow without retrying an unconditional overwrite', async ({ page }) => {
    const api = await installMockApi(page, {
      documents: [
        {
          content: '# Base\n',
          id: 'doc-daily',
          metadata: { title: 'Daily' },
          path: 'daily.md',
          version: 'v1',
        },
      ],
      rejectNextSaveAsStale: {
        content: '# Remote\n',
        version: 'v2',
      },
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Daily/ }).click();
    await expect(page.getByLabel('Plate markdown editor')).toContainText('Base');

    // Autosave attempts one conditional PUT; the stale rejection opens the
    // conflict dialog (focus trapped on the primary action) and is not retried.
    const editor = page.getByLabel('Plate markdown editor');
    await editor.click();
    await page.keyboard.press('End');
    await page.keyboard.type(' edit');

    await expect(page.getByRole('heading', { name: 'Local draft' })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Latest remote' })).toBeVisible();
    await expect(page.getByText('Path daily.md')).toBeVisible();
    await expect(page.getByText('Base "v1"')).toBeVisible();
    await expect(page.getByText('Latest "v2"')).toBeVisible();
    await expect(page.locator('pre').filter({ hasText: '# Base' })).toBeVisible();
    await expect(page.locator('pre').filter({ hasText: '# Remote' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Use remote' })).toBeFocused();
    await expect(page.locator('[aria-label="Save status"]')).toContainText('Stale');
    expect(api.saveHeaders).toEqual(['"v1"']);
  });

  test('searches and opens a server result from the keyboard', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: '# Daily\n',
          id: 'doc-daily',
          metadata: { title: 'Daily' },
          path: 'daily.md',
          version: 'v1',
        },
        {
          content: '# Guide\nSearchable body',
          id: 'doc-guide',
          metadata: { title: 'Guide' },
          path: 'docs/guide.md',
          version: 'v-guide',
        },
      ],
    });

    await page.goto('/');

    await page.getByRole('button', { name: 'Search' }).click();
    await page.getByRole('textbox', { name: 'Search' }).fill('guide');
    const results = page.getByRole('listbox', { name: 'Search results' });
    await expect(results.getByRole('option', { name: /Guide/ })).toBeVisible();

    await results.focus();
    await page.keyboard.press('Enter');

    await expect(page.getByLabel('Plate markdown editor')).toContainText('Searchable body');
    await expect(page).toHaveURL(/\/lib\/notes\/documents\/docs\/guide\.md$/);
  });

  test('renames a focused tree document from the keyboard', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: '# Daily\n',
          id: 'doc-daily',
          metadata: { title: 'Daily' },
          path: 'daily.md',
          version: 'v1',
        },
      ],
    });

    await page.goto('/');

    await expect(page.getByRole('treeitem', { name: /Daily/ })).toBeVisible();
    page.once('dialog', async (dialog) => {
      expect(dialog.message()).toBe('Move document to path');
      expect(dialog.defaultValue()).toBe('daily.md');
      await dialog.accept('archive/daily.md');
    });

    await page.getByRole('treeitem', { name: /Daily/ }).focus();
    await page.keyboard.press('F2');

    await expect(page.getByRole('treeitem', { name: /archive/ })).toBeVisible();
    await page.getByRole('treeitem', { name: /Daily/ }).click();
    await expect(page.getByLabel('Plate markdown editor')).toContainText('Daily');
    await expect(page).toHaveURL(/\/lib\/notes\/documents\/archive\/daily\.md$/);
  });

  test('deletes a document from the tree context menu', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: '# Daily\n',
          id: 'doc-daily',
          metadata: { title: 'Daily' },
          path: 'daily.md',
          version: 'v1',
        },
      ],
    });

    await page.goto('/');

    await page.getByRole('treeitem', { name: /Daily/ }).click();
    await expect(page.getByLabel('Plate markdown editor')).toContainText('Daily');

    page.once('dialog', async (dialog) => {
      expect(dialog.message()).toBe('Delete daily.md?');
      await dialog.accept();
    });
    await page.getByRole('treeitem', { name: /Daily/ }).focus();
    await page.keyboard.press('Shift+F10');
    await page.getByRole('menuitem', { name: 'Delete' }).click();

    await expect(page.getByRole('treeitem', { name: /Daily/ })).not.toBeVisible();
    await expect(page.getByText('No document open')).toBeVisible();
  });

  test('preserves edits across a reload', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: '# Server\n',
          id: 'doc-draft',
          metadata: { title: 'Draft' },
          path: 'draft.md',
          version: 'v-draft',
        },
      ],
    });

    await page.goto('/');

    await page.getByRole('treeitem', { name: /Draft/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('Server');
    await editor.click();
    await page.keyboard.press('End');
    await page.keyboard.type(' edited');

    await page.reload();

    // The edit survives a reload — via the server (autosave) or the local draft.
    await page.getByRole('treeitem', { name: /Draft/ }).click();
    await expect(page.getByLabel('Plate markdown editor')).toContainText('edited');
  });

  test('keeps a ten-thousand document tree virtualized and usable', async ({ page }) => {
    await installMockApi(page, {
      documents: Array.from({ length: 10_000 }, (_, index) => {
        const paddedIndex = index.toString().padStart(4, '0');
        return {
          content: `# Note ${paddedIndex}\n`,
          id: `doc-${paddedIndex}`,
          metadata: { title: `Note ${paddedIndex}` },
          path: `folder-${Math.floor(index / 1000)}/note-${paddedIndex}.md`,
          version: `v-${paddedIndex}`,
        };
      }),
    });

    await page.goto('/');

    await expect(page.getByRole('treeitem', { name: /folder-0/ })).toBeVisible();
    const renderedTreeItems = await page.getByRole('treeitem').count();
    expect(renderedTreeItems).toBeGreaterThan(0);
    expect(renderedTreeItems).toBeLessThan(200);

    await page.getByRole('button', { name: 'Search' }).click();
    await page.getByRole('textbox', { name: 'Search' }).fill('note-9999');
    const results = page.getByRole('listbox', { name: 'Search results' });
    await expect(results.getByRole('option', { name: /Note 9999/ })).toBeVisible();
    await results.focus();
    await page.keyboard.press('Enter');

    await expect(page.getByLabel('Plate markdown editor')).toContainText('Note 9999');
    await expect(page).toHaveURL(/\/lib\/notes\/documents\/folder-9\/note-9999\.md$/);
  });

  test('previews image and binary documents without opening the editor', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          byteSize: 2048,
          content: 'image-bytes',
          contentType: 'image/png',
          id: 'doc-photo',
          metadata: {},
          path: 'assets/photo.png',
          version: 'v-photo',
        },
        {
          byteSize: 4096,
          content: 'binary-bytes',
          contentHash: 'blake3-raw-bin',
          contentType: 'application/octet-stream',
          id: 'doc-raw',
          metadata: {},
          path: 'archives/raw.bin',
          version: 'v-raw',
        },
      ],
    });

    await page.goto('/');

    await page.getByRole('treeitem', { name: /photo\.png/ }).click();
    await expect(page.getByRole('img', { name: 'assets/photo.png preview' })).toHaveAttribute(
      'src',
      '/v1/libraries/notes/documents/assets/photo.png'
    );
    await expect(page.getByLabel('Plate markdown editor')).not.toBeVisible();

    await page.getByRole('treeitem', { name: /raw\.bin/ }).click();
    const binaryPreview = page.getByRole('region', { name: 'Binary document preview' });
    await expect(binaryPreview).toContainText('application/octet-stream');
    await expect(binaryPreview).toContainText('4 KB');
    await expect(binaryPreview).toContainText('Hash');
    await expect(binaryPreview).toContainText('blake3-raw-bin');
    await expect(page.getByRole('link', { name: 'Download' })).toHaveAttribute(
      'href',
      '/v1/libraries/notes/documents/archives/raw.bin'
    );
    await expect(page.getByLabel('Plate markdown editor')).not.toBeVisible();
  });

  test('navigates through wiki-links and backlinks', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: '# Daily\n\nSee [[Guide]].',
          id: 'doc-daily',
          metadata: { title: 'Daily' },
          path: 'daily.md',
          version: 'v-daily',
        },
        {
          content: '# Guide\n\nReference notes.',
          id: 'doc-guide',
          metadata: { title: 'Guide' },
          path: 'guide.md',
          version: 'v-guide',
        },
      ],
      links: {
        'daily.md': {
          outgoing: [
            link({
              src_doc_id: 'doc-daily',
              src_path: 'daily.md',
              src_version_id: 'v-daily',
              target_doc_id: 'doc-guide',
              target_path: 'guide.md',
              target_text: 'Guide',
            }),
          ],
        },
        'guide.md': {
          backlinks: [
            link({
              src_doc_id: 'doc-daily',
              src_path: 'daily.md',
              src_version_id: 'v-daily',
              target_doc_id: 'doc-guide',
              target_path: 'guide.md',
              target_text: 'Guide',
            }),
          ],
        },
      },
    });

    await page.goto('/');

    await page.getByRole('treeitem', { name: /Daily/ }).click();
    await page.getByRole('button', { name: 'guide.md' }).click();

    await expect(page.getByLabel('Plate markdown editor')).toContainText('Reference notes.');
    await expect(page).toHaveURL(/\/lib\/notes\/documents\/guide\.md$/);

    await page.getByRole('button', { name: 'daily.md' }).click();

    // `[[Guide]]` renders as a wiki-link chip in the editor, not literal text.
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('See');
    await expect(editor.getByTestId('wikilink')).toHaveText('Guide');
    await expect(page).toHaveURL(/\/lib\/notes\/documents\/daily\.md$/);
  });

  test('shows a floating formatting toolbar when text is selected', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: '# Heading\n\nSome body text to format.',
          id: 'doc-fmt',
          metadata: { title: 'Format' },
          path: 'format.md',
          version: 'v1',
        },
      ],
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Format/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('Some body text');

    await expect(page.getByRole('button', { name: 'Bold' })).toHaveCount(0);

    await editor.click();
    await page.keyboard.press('ControlOrMeta+a');

    await expect(page.getByRole('button', { name: 'Bold' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Italic' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Strikethrough' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Inline code' })).toBeVisible();

    // Each mark renders its formatting in the editor.
    await page.getByRole('button', { name: 'Strikethrough' }).click();
    await expect(editor.locator('s').first()).toBeVisible();
    await editor.click();
    await page.keyboard.press('ControlOrMeta+a');
    await page.getByRole('button', { name: 'Italic' }).click();
    await expect(editor.locator('em').first()).toBeVisible();
  });

  test('turns a block into a heading from the floating toolbar', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: '# Title\n\nA plain paragraph.',
          id: 'doc-blocks',
          metadata: { title: 'Blocks' },
          path: 'blocks.md',
          version: 'v1',
        },
      ],
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Blocks/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('A plain paragraph');

    await page.getByText('A plain paragraph', { exact: false }).click({ clickCount: 3 });
    await page.getByRole('button', { name: 'Turn into' }).click();
    await page.getByRole('menuitem', { name: 'Heading 2' }).click();
    await expect(editor.locator('h2')).toHaveText('A plain paragraph.');
  });

  test('auto-formats markdown shortcuts into blocks while typing', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: '',
          id: 'doc-autoformat',
          metadata: { title: 'Autoformat' },
          path: 'autoformat.md',
          version: 'v1',
        },
      ],
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Autoformat/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await editor.click();

    await page.keyboard.type('# Big heading');
    await expect(editor.locator('h1')).toHaveText('Big heading');

    await page.keyboard.press('Enter');
    await page.keyboard.type('###### Small heading');
    await expect(editor.locator('h6')).toHaveText('Small heading');

    await page.keyboard.press('Enter');
    await page.keyboard.type('> A quote');
    await expect(editor.locator('blockquote')).toHaveText('A quote');
  });

  test('toggles bullet and numbered lists from the floating toolbar', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: 'A list item.',
          id: 'doc-lists',
          metadata: { title: 'Lists' },
          path: 'lists.md',
          version: 'v1',
        },
      ],
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Lists/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('A list item');

    await page.getByText('A list item', { exact: false }).click({ clickCount: 3 });
    const bullet = page.getByRole('button', { name: 'Bullet list' });
    await bullet.click();
    await expect(bullet).toHaveAttribute('aria-pressed', 'true');

    const numbered = page.getByRole('button', { name: 'Numbered list' });
    await numbered.click();
    await expect(numbered).toHaveAttribute('aria-pressed', 'true');
    await expect(bullet).toHaveAttribute('aria-pressed', 'false');
  });

  test('creates a to-do list with a checkbox via markdown autoformat', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: '',
          id: 'doc-todo',
          metadata: { title: 'Todos' },
          path: 'todos.md',
          version: 'v1',
        },
      ],
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Todos/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await editor.click();

    // GitHub-style `[ ]` (space inside the brackets) creates an unchecked to-do.
    await page.keyboard.type('[ ] Buy milk');
    const checkbox = editor.getByRole('checkbox', { name: 'Toggle to-do' });
    await expect(checkbox).toBeVisible();
    await expect(checkbox).not.toBeChecked();
    await expect(editor).toContainText('Buy milk');

    await checkbox.check();
    await expect(checkbox).toBeChecked();
  });

  test('creates a checked to-do via [x] autoformat', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: '',
          id: 'doc-todo-x',
          metadata: { title: 'TodosX' },
          path: 'todos-x.md',
          version: 'v1',
        },
      ],
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /TodosX/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await editor.click();

    await page.keyboard.type('[x] Done thing');
    const checkbox = editor.getByRole('checkbox', { name: 'Toggle to-do' });
    await expect(checkbox).toBeVisible();
    await expect(checkbox).toBeChecked();
    await expect(editor).toContainText('Done thing');
  });

  test('shows a drag handle for editor blocks', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: '# Title\n\nFirst paragraph.\n\nSecond paragraph.',
          id: 'doc-drag',
          metadata: { title: 'Draggy' },
          path: 'draggy.md',
          version: 'v1',
        },
      ],
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Draggy/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('First paragraph');

    const para = page.getByText('First paragraph.', { exact: false });
    await para.hover();
    const handle = editor.getByRole('button', { name: 'Drag to move block' }).first();
    await expect(handle).toBeVisible();

    // The document tree's own drag-and-drop still works alongside the editor's
    // (shared react-dnd manager — no second HTML5 backend crash).
    await expect(page.getByRole('treeitem', { name: /Draggy/ })).toBeVisible();
  });

  test('reorders blocks by dragging the block handle', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: 'Alpha block\n\nBravo block\n\nCharlie block',
          id: 'doc-reorder',
          metadata: { title: 'Reorder' },
          path: 'reorder.md',
          version: 'v1',
        },
      ],
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Reorder/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('Charlie block');

    const blockOrder = () => editor.evaluate((el) => (el as HTMLElement).innerText.replace(/\n+/g, '|'));
    expect(await blockOrder()).toBe('Alpha block|Bravo block|Charlie block');

    await page.getByText('Charlie block', { exact: false }).hover();
    const charlieHandle = editor.getByRole('button', { name: 'Drag to move block' }).last();
    await charlieHandle.dragTo(page.getByText('Alpha block', { exact: false }));

    await expect.poll(blockOrder).toBe('Charlie block|Alpha block|Bravo block');
  });

  // Regression: with the editor's centered layout, the handle must sit inside
  // the block's drop target. If it's out in the gutter, dragging straight up/down
  // (cursor staying at the handle's x) never hovers a drop target and the drop
  // never fires. This drags vertically at the handle's x to catch that.
  test('reorders when dragging straight down the handle gutter', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: 'Alpha block\n\nBravo block\n\nCharlie block',
          id: 'doc-gutter',
          metadata: { title: 'Gutter' },
          path: 'gutter.md',
          version: 'v1',
        },
      ],
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Gutter/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('Charlie block');

    const firstBox = await editor.locator('[data-block-id]').first().boundingBox();
    await page.getByText('Charlie block', { exact: false }).hover();
    const handleBox = await editor.getByRole('button', { name: 'Drag to move block' }).last().boundingBox();
    if (!handleBox || !firstBox) throw new Error('missing bounding boxes');

    const x = handleBox.x + handleBox.width / 2;
    await page.mouse.move(x, handleBox.y + handleBox.height / 2);
    await page.mouse.down();
    for (let step = 1; step <= 10; step += 1) {
      await page.mouse.move(x, handleBox.y + ((firstBox.y - handleBox.y) * step) / 10, { steps: 2 });
    }
    await page.mouse.move(x, firstBox.y + 3);
    await page.mouse.up();

    await expect
      .poll(() => editor.evaluate((el) => (el as HTMLElement).innerText.replace(/\n+/g, '|')))
      .toBe('Charlie block|Alpha block|Bravo block');
  });

  test('applies underline from the floating toolbar', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: 'Underline me',
          id: 'doc-underline',
          metadata: { title: 'Underliney' },
          path: 'underliney.md',
          version: 'v1',
        },
      ],
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Underliney/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('Underline me');

    await page.getByText('Underline me', { exact: false }).click({ clickCount: 3 });
    const underline = page.getByRole('button', { name: 'Underline' });
    await underline.click();

    await expect(underline).toHaveAttribute('aria-pressed', 'true');
    await expect(editor.locator('u, [style*="underline"]').first()).toBeVisible();
  });

  test('applies superscript and subscript from the floating toolbar', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: 'Format me',
          id: 'doc-sup',
          metadata: { title: 'SupSub' },
          path: 'supsub.md',
          version: 'v1',
        },
      ],
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /SupSub/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('Format me');

    await page.getByText('Format me', { exact: false }).click({ clickCount: 3 });
    const superscript = page.getByRole('button', { name: 'Superscript' });
    await superscript.click();
    await expect(superscript).toHaveAttribute('aria-pressed', 'true');

    // Toggling superscript off then subscript on (they're mutually exclusive marks).
    await superscript.click();
    const subscript = page.getByRole('button', { name: 'Subscript' });
    await subscript.click();
    await expect(subscript).toHaveAttribute('aria-pressed', 'true');
    await expect(editor).toContainText('Format me');
  });

  test('turns a paragraph into a to-do from the floating toolbar', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: 'Walk the dog',
          id: 'doc-todobar',
          metadata: { title: 'TodoBar' },
          path: 'todobar.md',
          version: 'v1',
        },
      ],
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /TodoBar/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('Walk the dog');

    await page.getByText('Walk the dog', { exact: false }).click({ clickCount: 3 });
    await page.getByRole('button', { name: 'To-do list' }).click();

    const checkbox = editor.getByRole('checkbox', { name: 'Toggle to-do' });
    await expect(checkbox).toBeVisible();
    await expect(checkbox).not.toBeChecked();
    await expect(editor).toContainText('Walk the dog');
  });

  test('opens a block menu from the handle with turn-into, duplicate, delete', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: 'Alpha block\n\nBravo block',
          id: 'doc-blockmenu',
          metadata: { title: 'BlockMenu' },
          path: 'blockmenu.md',
          version: 'v1',
        },
      ],
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /BlockMenu/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('Alpha block');
    const blockText = () => editor.evaluate((el) => (el as HTMLElement).innerText.replace(/\n+/g, '|'));

    // Click the handle (a native drag would never fire click) to open the menu.
    await page.getByText('Alpha block', { exact: false }).hover();
    await editor.getByRole('button', { name: 'Drag to move block' }).first().click();
    const menu = page.getByRole('menu', { name: 'Block actions' });
    await expect(menu).toBeVisible();
    await expect(menu.getByRole('menuitem', { name: 'Turn into' })).toBeVisible();

    await menu.getByRole('menuitem', { name: 'Duplicate' }).click();
    await expect.poll(blockText).toBe('Alpha block|Alpha block|Bravo block');

    // Turn into opens a hover sub-list that includes the list options.
    await page.getByText('Alpha block', { exact: false }).first().hover();
    await editor.getByRole('button', { name: 'Drag to move block' }).first().click();
    const reopened = page.getByRole('menu', { name: 'Block actions' });
    await reopened.getByRole('menuitem', { name: 'Turn into' }).hover();
    await expect(reopened.getByRole('menuitem', { name: 'Bulleted list' })).toBeVisible();
    await expect(reopened.getByRole('menuitem', { name: 'To-do list' })).toBeVisible();
    await reopened.getByRole('menuitem', { name: 'Heading 1' }).click();
    await expect(editor.locator('h1')).toHaveText('Alpha block');

    await page.getByText('Alpha block', { exact: false }).first().hover();
    await editor.getByRole('button', { name: 'Drag to move block' }).first().click();
    await page.getByRole('menu', { name: 'Block actions' }).getByRole('menuitem', { name: 'Delete' }).click();
    await expect.poll(blockText).toBe('Alpha block|Bravo block');
  });

  test('turns a block into a code block from the Turn into dropdown', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: 'A plain paragraph.',
          id: 'doc-code',
          metadata: { title: 'Codey' },
          path: 'codey.md',
          version: 'v1',
        },
      ],
    });

    await page.goto('/');
    await page.getByRole('treeitem', { name: /Codey/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    await expect(editor).toContainText('A plain paragraph');

    await page.getByText('A plain paragraph', { exact: false }).click({ clickCount: 3 });
    await page.getByRole('button', { name: 'Turn into' }).click();
    await page.getByRole('menuitem', { name: 'Code' }).click();
    await expect(editor.locator('.slate-code_block')).toContainText('A plain paragraph');

    await editor.getByRole('button', { name: 'Copy code' }).click();
    await expect(editor.getByRole('button', { name: 'Copied' })).toBeVisible();
  });

  test('diffs selected historical versions from the version pane', async ({ page }) => {
    await installMockApi(page, {
      documents: [
        {
          content: '# Three\n',
          id: 'doc-compare',
          metadata: { title: 'Compare' },
          path: 'compare.md',
          version: 'v3',
        },
      ],
      versions: {
        'compare.md': [
          { id: 'v3', content: '# Three\n', created_at: '2026-05-29T12:00:00Z' },
          { id: 'v2', content: '# Two\n', created_at: '2026-05-28T12:00:00Z' },
          { id: 'v1', content: '# One\n', created_at: '2026-05-27T12:00:00Z' },
        ],
      },
    });

    await page.goto('/');

    await page.getByRole('treeitem', { name: /Compare/ }).click();
    await page.getByRole('tab', { name: 'Versions' }).click();
    await page.getByLabel('View version v1').click();

    await expect(page.getByText('# One', { exact: true })).toBeVisible();
    await expect(page.locator('pre').filter({ hasText: '+# Three' })).toBeVisible();

    await page.getByLabel('Compare version against').selectOption('v2');

    await expect(page.locator('pre').filter({ hasText: '+# Two' })).toBeVisible();
    await expect(page.locator('pre').filter({ hasText: '+# Three' })).not.toBeVisible();
  });

  test('restores an older version from the version pane', async ({ page }) => {
    const api = await installMockApi(page, {
      documents: [
        {
          content: '# Current\n',
          id: 'doc-versioned',
          metadata: { title: 'Versioned' },
          path: 'versioned.md',
          version: 'v-current',
        },
      ],
      versions: {
        'versioned.md': [
          { id: 'v-current', content: '# Current\n', created_at: '2026-05-29T12:00:00Z' },
          { id: 'v-old', content: '# Old\n', created_at: '2026-05-28T12:00:00Z' },
        ],
      },
    });

    await page.goto('/');

    await page.getByRole('treeitem', { name: /Versioned/ }).click();
    await page.getByRole('tab', { name: 'Versions' }).click();
    await page.getByLabel('Restore version v-old').click();

    await expect(page.getByLabel('Plate markdown editor')).toContainText('Old');
    expect(api.restoredVersions).toEqual(['versioned.md:v-old']);
  });

  test('refreshes the open document from SSE change events', async ({ page }) => {
    await installControllableEventSource(page);
    const api = await installMockApi(page, {
      documents: [
        {
          content: '# Initial\n',
          id: 'doc-daily',
          metadata: { title: 'Daily' },
          path: 'daily.md',
          version: 'v1',
        },
      ],
    });

    await page.goto('/');
    await expect(page.getByRole('treeitem', { name: /Daily/ })).toBeVisible();

    await page.getByRole('treeitem', { name: /Daily/ }).click();
    await expect(page.getByLabel('Plate markdown editor')).toContainText('Initial');

    api.documents.set('daily.md', {
      ...api.documents.get('daily.md')!,
      content: '# External\n',
      version: 'v2',
    });
    await emitMockEventSource(page, 'doc.changed', {
      type: 'doc.changed',
      library: 'notes',
      path: 'daily.md',
    });

    await expect(page.getByLabel('Plate markdown editor')).toContainText('External');
  });

  test('resolves a Git conflict record from the conflict workflow', async ({ page }) => {
    const api = await installMockApi(page, {
      conflicts: [
        {
          conflict_path: 'conflict.sibling.md',
          id: 'conflict-git',
          ours_version_id: 'ours',
          path: 'conflict.md',
          theirs_version_id: 'theirs',
        },
      ],
      documents: [
        {
          content: '# Head\n',
          id: 'doc-conflict',
          metadata: { title: 'Conflict' },
          path: 'conflict.md',
          version: 'head',
        },
        {
          content: '# Theirs\n',
          id: 'doc-conflict-sibling',
          metadata: { title: 'Conflict sibling' },
          path: 'conflict.sibling.md',
          version: 'theirs',
        },
      ],
      versions: {
        'conflict.md': [{ id: 'ours', content: '# Ours\n' }],
        'conflict.sibling.md': [{ id: 'theirs', content: '# Theirs\n' }],
      },
    });

    await page.goto('/');

    await page.getByRole('tab', { name: 'Conflicts' }).click();
    await expect(page.getByText('conflict.md open')).toBeVisible();
    await expect(page.getByText('Sibling conflict.sibling.md')).toBeVisible();
    await page.getByLabel('Open conflict conflict-git').click();

    const dialog = page.getByRole('dialog', { name: 'Resolve conflict' });
    await expect(dialog.locator('pre').filter({ hasText: '# Ours' })).toBeVisible();
    await expect(dialog.locator('pre').filter({ hasText: '# Theirs' })).toBeVisible();
    await dialog.getByRole('button', { name: 'Use theirs' }).click();

    await expect(dialog).not.toBeVisible();
    await expect(page.getByText('conflict.md open')).not.toBeVisible();
    await page.locator('[data-tree-path="conflict.md"]').click();
    await expect(page.getByLabel('Plate markdown editor')).toContainText('Theirs');
    expect(api.resolvedConflicts).toEqual(['conflict-git']);
    expect(api.saveHeaders).toContain('"head"');
  });

  test('resolves a Git conflict using keyboard-only navigation', async ({ page }) => {
    const api = await installMockApi(page, {
      conflicts: [
        {
          conflict_path: 'conflict.sibling.md',
          id: 'conflict-keyboard',
          ours_version_id: 'ours',
          path: 'conflict.md',
          theirs_version_id: 'theirs',
        },
      ],
      documents: [
        {
          content: '# Head\n',
          id: 'doc-conflict',
          metadata: { title: 'Conflict' },
          path: 'conflict.md',
          version: 'head',
        },
        {
          content: '# Theirs\n',
          id: 'doc-conflict-sibling',
          metadata: { title: 'Conflict sibling' },
          path: 'conflict.sibling.md',
          version: 'theirs',
        },
      ],
      versions: {
        'conflict.md': [{ id: 'ours', content: '# Ours\n' }],
        'conflict.sibling.md': [{ id: 'theirs', content: '# Theirs\n' }],
      },
    });

    await page.goto('/');

    await page.getByRole('tab', { name: 'Conflicts' }).focus();
    await page.keyboard.press('Enter');
    await expect(page.getByText('conflict.md open')).toBeVisible();

    await page.getByLabel('Open conflict conflict-keyboard').focus();
    await page.keyboard.press('Enter');

    const dialog = page.getByRole('dialog', { name: 'Resolve conflict' });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole('button', { name: 'Close' })).toBeFocused();

    await page.keyboard.press('Tab');
    await expect(dialog.getByLabel('Manual resolution')).toBeFocused();
    await page.keyboard.press('Tab');
    await expect(dialog.getByRole('button', { name: 'Use ours' })).toBeFocused();
    await page.keyboard.press('Tab');
    await expect(dialog.getByRole('button', { name: 'Use theirs' })).toBeFocused();
    await page.keyboard.press('Enter');

    await expect(dialog).not.toBeVisible();
    await expect(page.getByText('conflict.md open')).not.toBeVisible();

    await page.locator('[data-tree-path="conflict.md"]').focus();
    await page.keyboard.press('Enter');
    await expect(page.getByLabel('Plate markdown editor')).toContainText('Theirs');
    expect(api.resolvedConflicts).toEqual(['conflict-keyboard']);
    expect(api.saveHeaders).toContain('"head"');
  });
});

async function disableEventSource(page: Page) {
  await page.addInitScript(() => {
    class DisabledEventSource {
      onerror: ((event: Event) => void) | null = null;
      onopen: ((event: Event) => void) | null = null;

      constructor() {
        window.setTimeout(() => this.onerror?.(new Event('error')), 0);
      }

      addEventListener() {}
      close() {}
      removeEventListener() {}
    }

    Object.defineProperty(window, 'EventSource', {
      configurable: true,
      value: DisabledEventSource,
    });
  });
}

async function installControllableEventSource(page: Page) {
  await page.addInitScript(() => {
    type TestEventSourceInstance = EventSource & {
      listeners: Map<string, Array<(event: MessageEvent) => void>>;
    };
    type TestEventWindow = Window & {
      __quarryEmitSse?: (type: string, payload: Record<string, unknown>) => void;
      __quarryEventSources?: TestEventSourceInstance[];
    };
    const testWindow = window as TestEventWindow;
    testWindow.__quarryEventSources = [];

    function TestEventSource(this: TestEventSourceInstance, url: string) {
      Object.assign(this, { listeners: new Map(), onerror: null, onopen: null, url });
      testWindow.__quarryEventSources?.push(this);
      window.queueMicrotask(() => this.onopen?.(new Event('open')));
    }

    TestEventSource.prototype.addEventListener = function (
      this: TestEventSourceInstance,
      type: string,
      listener: (event: MessageEvent) => void
    ) {
      this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
    };
    TestEventSource.prototype.removeEventListener = function (
      this: TestEventSourceInstance,
      type: string,
      listener: (event: MessageEvent) => void
    ) {
      this.listeners.set(
        type,
        (this.listeners.get(type) ?? []).filter((existing) => existing !== listener)
      );
    };
    TestEventSource.prototype.close = function (this: TestEventSourceInstance) {
      testWindow.__quarryEventSources = (testWindow.__quarryEventSources ?? []).filter(
        (source) => source !== this
      );
    };

    Object.defineProperty(window, 'EventSource', {
      configurable: true,
      value: TestEventSource,
    });
    Object.defineProperty(window, '__quarryEmitSse', {
      configurable: true,
      value: (type: string, payload: Record<string, unknown>) => {
        for (const source of testWindow.__quarryEventSources ?? []) {
          for (const listener of source.listeners.get(type) ?? []) {
            listener(new MessageEvent(type, { data: JSON.stringify(payload) }));
          }
        }
      },
    });
  });
}

async function emitMockEventSource(page: Page, type: string, payload: Record<string, unknown>) {
  await page.evaluate(
    ({ eventType, eventPayload }) => {
      (window as Window & {
        __quarryEmitSse?: (type: string, payload: Record<string, unknown>) => void;
      }).__quarryEmitSse?.(eventType, eventPayload);
    },
    { eventPayload: payload, eventType: type }
  );
}

async function expectNoAxeViolations(page: Page, context: string) {
  const results = await new AxeBuilder({ page }).analyze();
  expect(results.violations, `${context} has accessibility violations`).toEqual([]);
}

async function runCommand(page: Page, query: string, command: string) {
  await page.keyboard.press('ControlOrMeta+K');
  const palette = page.getByRole('dialog', { name: 'Command palette' });
  await expect(palette).toBeVisible();
  await page.getByRole('combobox', { name: 'Command palette' }).fill(query);
  await palette.getByText(command, { exact: true }).click();
  await expect(palette).not.toBeVisible();
}

async function installMockApi(
  page: Page,
  options: {
    conflicts?: MockConflict[];
    documents: MockDocument[];
    documentsByLibrary?: Record<string, MockDocument[]>;
    libraries?: MockLibrary[];
    links?: Record<string, { backlinks?: MockLink[]; outgoing?: MockLink[] }>;
    rejectNextSaveAsStale?: Pick<MockDocument, 'content' | 'version'>;
    versions?: Record<string, MockVersion[]>;
  }
) {
  const libraries = options.libraries ?? [{ id: 'lib-notes', slug: 'notes' }];
  const documentsByLibrary = new Map<string, Map<string, MockDocument>>();
  const initialDocumentsByLibrary = options.documentsByLibrary ?? {};

  for (const library of libraries) {
    const libraryDocuments = initialDocumentsByLibrary[library.slug] ?? (library.slug === 'notes' ? options.documents : []);
    documentsByLibrary.set(
      library.slug,
      new Map(libraryDocuments.map((document) => [document.path, { ...document }]))
    );
  }

  const defaultDocuments =
    documentsByLibrary.get('notes') ?? documentsByLibrary.get(libraries[0]?.slug ?? '') ?? new Map<string, MockDocument>();

  const state = {
    conflicts: options.conflicts ?? [],
    createHeaders: [] as string[],
    deletedDocuments: [] as string[],
    documents: defaultDocuments,
    documentsByLibrary,
    links: options.links ?? {},
    rejectNextSaveAsStale: options.rejectNextSaveAsStale,
    resolvedConflicts: [] as string[],
    restoredVersions: [] as string[],
    saveHeaders: [] as string[],
    savedBodies: [] as { body: string; path: string }[],
    versions: options.versions ?? {},
  };

  await page.route('**/v1/**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = decodeURIComponent(url.pathname);

    if (path === '/v1/libraries') {
      await route.fulfill({
        json: libraries.map((library) => ({ ...library, created_at: 'now', settings: {} })),
      });
      return;
    }

    const libraryEndpoint = libraryPathFromEndpoint(path);
    const libraryDocuments = libraryEndpoint ? state.documentsByLibrary.get(libraryEndpoint.library) : undefined;

    if (libraryEndpoint?.resourcePath === 'documents' && request.method() === 'GET') {
      await route.fulfill({ json: Array.from(libraryDocuments?.values() ?? []).map(documentStub) });
      return;
    }

    const conflictResolveId = conflictResolveFromEndpoint(path);
    if (conflictResolveId && request.method() === 'POST') {
      const conflict = state.conflicts.find((entry) => entry.id === conflictResolveId);
      if (!conflict) {
        await notFound(route);
        return;
      }
      state.resolvedConflicts.push(conflictResolveId);
      await route.fulfill({ json: conflictRecord(conflict, true) });
      return;
    }

    if (libraryEndpoint?.resourcePath === 'conflicts') {
      await route.fulfill({
        json: state.conflicts
          .filter((conflict) => !state.resolvedConflicts.includes(conflict.id))
          .map((conflict) => conflictRecord(conflict)),
      });
      return;
    }

    if (libraryEndpoint?.resourcePath === 'git/peers') {
      await route.fulfill({ json: [] });
      return;
    }

    if (libraryEndpoint?.resourcePath.startsWith('search/suggest')) {
      await route.fulfill({ json: [] });
      return;
    }

    if (libraryEndpoint?.resourcePath.startsWith('search')) {
      const query = url.searchParams.get('q')?.trim().toLowerCase() ?? '';
      await route.fulfill({
        json: {
          results: Array.from(libraryDocuments?.values() ?? [])
            .filter((document) => documentMatchesSearch(document, query))
            .map(searchResult),
          cursor: null,
        },
      });
      return;
    }

    if (libraryEndpoint?.resourcePath.startsWith('graph')) {
      await route.fulfill({ json: { nodes: [], edges: [], truncated: false } });
      return;
    }

    if (path.endsWith('/outgoing-links') || path.endsWith('/backlinks')) {
      const documentPath = documentPathFromNestedEndpoint(path);
      const direction = path.endsWith('/backlinks') ? 'backlinks' : 'outgoing';
      await route.fulfill({ json: { path: documentPath, links: state.links[documentPath]?.[direction] ?? [] } });
      return;
    }

    const restorePath = documentVersionRestoreFromEndpoint(path);
    if (restorePath && request.method() === 'POST') {
      const documents = state.documentsByLibrary.get(restorePath.library);
      const document = documents?.get(restorePath.documentPath);
      const versionToRestore = state.versions[restorePath.documentPath]?.find(
        (version) => version.id === restorePath.version
      );
      if (!documents || !document || !versionToRestore) {
        await notFound(route);
        return;
      }

      state.restoredVersions.push(`${restorePath.documentPath}:${restorePath.version}`);
      const restoredDocument: MockDocument = {
        ...document,
        content: versionToRestore.content,
        version: 'v-restored',
      };
      documents.set(restorePath.documentPath, restoredDocument);
      state.versions[restorePath.documentPath] = [
        { id: restoredDocument.version, content: restoredDocument.content, created_at: '2026-05-29T13:00:00Z' },
        ...(state.versions[restorePath.documentPath] ?? []),
      ];
      await route.fulfill({
        headers: { ETag: `"${restoredDocument.version}"` },
        json: writeOutcome(restoredDocument),
      });
      return;
    }

    const diffPath = documentVersionDiffFromEndpoint(path);
    if (diffPath && request.method() === 'GET') {
      const document = state.documentsByLibrary.get(diffPath.library)?.get(diffPath.documentPath);
      const versions = state.versions[diffPath.documentPath] ?? [];
      const baseVersion = versions.find((entry) => entry.id === diffPath.version);
      const againstVersionId = url.searchParams.get('against');
      const againstVersion = againstVersionId ? versions.find((entry) => entry.id === againstVersionId) : undefined;
      const againstContent = againstVersion?.content ?? document?.content;
      const againstId = againstVersion?.id ?? document?.version;
      if (!baseVersion || !againstContent || !againstId) {
        await notFound(route);
        return;
      }

      await route.fulfill({
        json: {
          base_version_id: baseVersion.id,
          against_version_id: againstId,
          unified_diff: mockUnifiedDiff(baseVersion.content, againstContent),
        },
      });
      return;
    }

    const versionPath = documentVersionFromEndpoint(path);
    if (versionPath && request.method() === 'GET') {
      const document = state.documentsByLibrary.get(versionPath.library)?.get(versionPath.documentPath);
      const version = state.versions[versionPath.documentPath]?.find((entry) => entry.id === versionPath.version);
      if (!document || !version) {
        await notFound(route);
        return;
      }
      await route.fulfill({
        json: { version: versionRecord(document, version), content: version.content },
      });
      return;
    }

    const versionsPath = documentVersionsFromEndpoint(path);
    if (versionsPath) {
      const document = state.documentsByLibrary.get(versionsPath.library)?.get(versionsPath.documentPath);
      const versions = state.versions[versionsPath.documentPath] ?? [];
      await route.fulfill({ json: document ? versions.map((version) => versionRecord(document, version)) : [] });
      return;
    }

    const movePath = documentPathFromMoveEndpoint(path);
    if (movePath && request.method() === 'POST') {
      const documents = state.documentsByLibrary.get(movePath.library);
      const document = documents?.get(movePath.documentPath);
      if (!documents || !document) {
        await notFound(route);
        return;
      }

      const body = request.postDataJSON() as { to_path?: string };
      const toPath = body.to_path;
      if (!toPath) {
        await route.fulfill({ body: 'missing to_path', status: 400 });
        return;
      }

      const movedDocument: MockDocument = {
        ...document,
        path: toPath,
        version: 'v-moved',
      };
      documents.delete(movePath.documentPath);
      documents.set(toPath, movedDocument);
      await route.fulfill({
        headers: { ETag: `"${movedDocument.version}"` },
        json: writeOutcome(movedDocument),
      });
      return;
    }

    const documentPath = documentPathFromDocumentEndpoint(path);
    if (documentPath && request.method() === 'DELETE') {
      const documents = state.documentsByLibrary.get(documentPath.library) ?? state.documents;
      if (!documents.has(documentPath.documentPath)) {
        await notFound(route);
        return;
      }

      documents.delete(documentPath.documentPath);
      state.deletedDocuments.push(documentPath.documentPath);
      await route.fulfill({
        json: {
          actor: null,
          committed_at: 'now',
          created_at: 'now',
          id: `tx-delete-${documentPath.documentPath}`,
          library_id: `lib-${documentPath.library}`,
          message: null,
          provenance: {},
          source: 'rest',
          state: 'committed',
        },
      });
      return;
    }

    if (documentPath && request.method() === 'GET') {
      const document = state.documentsByLibrary.get(documentPath.library)?.get(documentPath.documentPath);
      if (!document) {
        await notFound(route);
        return;
      }
      await route.fulfill({
        body: document.content,
        headers: { ETag: `"${document.version}"`, 'content-type': documentContentType(document) },
      });
      return;
    }

    if (documentPath && request.method() === 'PUT') {
      const ifNoneMatch = request.headers()['if-none-match'];
      const ifMatch = request.headers()['if-match'];
      if (ifNoneMatch) state.createHeaders.push(ifNoneMatch);
      if (ifMatch) state.saveHeaders.push(ifMatch);
      state.savedBodies.push({ body: request.postData() ?? '', path: documentPath.documentPath });

      if (state.rejectNextSaveAsStale) {
        const documents = state.documentsByLibrary.get(documentPath.library) ?? state.documents;
        const current = documents.get(documentPath.documentPath);
        documents.set(documentPath.documentPath, {
          content: state.rejectNextSaveAsStale.content,
          id: current?.id ?? `doc-${documentPath.documentPath}`,
          metadata: current?.metadata,
          path: documentPath.documentPath,
          version: state.rejectNextSaveAsStale.version,
        });
        state.rejectNextSaveAsStale = undefined;
        await route.fulfill({
          json: { error: 'precondition failed' },
          status: 412,
        });
        return;
      }

      const nextVersion = ifNoneMatch ? 'v-new' : 'v-saved';
      const documents = state.documentsByLibrary.get(documentPath.library) ?? state.documents;
      const document: MockDocument = {
        content: request.postData() ?? '',
        id: documents.get(documentPath.documentPath)?.id ?? `doc-${documentPath.documentPath}`,
        metadata: documents.get(documentPath.documentPath)?.metadata ?? {},
        path: documentPath.documentPath,
        version: nextVersion,
      };
      documents.set(documentPath.documentPath, document);
      await route.fulfill({
        headers: { ETag: `"${document.version}"` },
        json: writeOutcome(document),
      });
      return;
    }

    await notFound(route);
  });

  return {
    ...state,
    // The body of the most recent PUT to `path` (the saved markdown), or '' if
    // that path hasn't been saved yet. `savedBodies` is mutated in place, so the
    // spread keeps the live array reference — poll-friendly.
    lastSavedBody(path: string): string {
      return state.savedBodies.findLast((entry) => entry.path === path)?.body ?? '';
    },
  };
}

function libraryPathFromEndpoint(path: string) {
  const prefix = '/v1/libraries/';
  if (!path.startsWith(prefix)) return null;
  const remainingPath = path.slice(prefix.length);
  const separatorIndex = remainingPath.indexOf('/');
  if (separatorIndex === -1) {
    return { library: remainingPath, resourcePath: '' };
  }
  return {
    library: remainingPath.slice(0, separatorIndex),
    resourcePath: remainingPath.slice(separatorIndex + 1),
  };
}

function documentPathFromDocumentEndpoint(path: string) {
  const endpoint = libraryPathFromEndpoint(path);
  const prefix = 'documents/';
  if (!endpoint?.resourcePath.startsWith(prefix)) return null;
  const documentPath = endpoint.resourcePath.slice(prefix.length);
  if (
    documentPath.endsWith('/backlinks') ||
    documentPath.endsWith('/outgoing-links') ||
    documentPath.endsWith('/versions') ||
    documentPath.endsWith('/move')
  ) {
    return null;
  }
  return { documentPath, library: endpoint.library };
}

function documentPathFromMoveEndpoint(path: string) {
  const endpoint = libraryPathFromEndpoint(path);
  const prefix = 'documents/';
  const suffix = '/move';
  if (!endpoint?.resourcePath.startsWith(prefix) || !endpoint.resourcePath.endsWith(suffix)) return null;
  return {
    documentPath: endpoint.resourcePath.slice(prefix.length, -suffix.length),
    library: endpoint.library,
  };
}

function documentVersionsFromEndpoint(path: string) {
  const endpoint = libraryPathFromEndpoint(path);
  const prefix = 'documents/';
  const suffix = '/versions';
  if (!endpoint?.resourcePath.startsWith(prefix) || !endpoint.resourcePath.endsWith(suffix)) return null;
  return {
    documentPath: endpoint.resourcePath.slice(prefix.length, -suffix.length),
    library: endpoint.library,
  };
}

function documentVersionFromEndpoint(path: string) {
  const endpoint = libraryPathFromEndpoint(path);
  const prefix = 'documents/';
  if (!endpoint?.resourcePath.startsWith(prefix)) return null;
  const marker = '/versions/';
  const markerIndex = endpoint.resourcePath.lastIndexOf(marker);
  if (markerIndex === -1 || endpoint.resourcePath.endsWith('/restore') || endpoint.resourcePath.includes('/diff')) {
    return null;
  }
  return {
    documentPath: endpoint.resourcePath.slice(prefix.length, markerIndex),
    library: endpoint.library,
    version: endpoint.resourcePath.slice(markerIndex + marker.length),
  };
}

function documentVersionRestoreFromEndpoint(path: string) {
  const suffix = '/restore';
  if (!path.endsWith(suffix)) return null;
  const versionPath = documentVersionFromEndpoint(path.slice(0, -suffix.length));
  return versionPath;
}

function documentVersionDiffFromEndpoint(path: string) {
  const suffix = '/diff';
  if (!path.endsWith(suffix)) return null;
  return documentVersionFromEndpoint(path.slice(0, -suffix.length));
}

function conflictResolveFromEndpoint(path: string) {
  const endpoint = libraryPathFromEndpoint(path);
  const prefix = 'conflicts/';
  const suffix = '/resolve';
  if (!endpoint?.resourcePath.startsWith(prefix) || !endpoint.resourcePath.endsWith(suffix)) return null;
  return endpoint.resourcePath.slice(prefix.length, -suffix.length);
}

function documentPathFromNestedEndpoint(path: string) {
  const endpoint = libraryPathFromEndpoint(path);
  return endpoint?.resourcePath.replace(/^documents\//, '').replace(/\/(?:backlinks|outgoing-links|versions)$/, '') ?? '';
}

function documentStub(document: MockDocument) {
  return {
    id: document.id,
    path: document.path,
    head_version_id: document.version,
    content_type: documentContentType(document),
    byte_size: documentByteSize(document),
    content_hash: document.contentHash ?? null,
    metadata: document.metadata ?? {},
    updated_at: 'now',
  };
}

function documentMatchesSearch(document: MockDocument, query: string) {
  if (!query) return false;
  const title = typeof document.metadata?.title === 'string' ? document.metadata.title : document.path;
  return [document.path, title, document.content].some((value) => value.toLowerCase().includes(query));
}

function searchResult(document: MockDocument) {
  const title = typeof document.metadata?.title === 'string' ? document.metadata.title : document.path;
  return {
    document_id: document.id,
    path: document.path,
    title,
    content_type: documentContentType(document),
    score: 1,
    snippet: document.content,
    matched_fields: ['body'],
    head_version_id: document.version,
  };
}

function versionRecord(document: MockDocument, version: MockVersion) {
  return {
    id: version.id,
    document_id: document.id,
    tx_id: `tx-${version.id}`,
    transaction_source: 'rest',
    transaction_actor: null,
    transaction_message: null,
    transaction_provenance: {},
    content_hash: document.contentHash ?? null,
    inline_content: null,
    metadata: document.metadata ?? {},
    content_type: documentContentType(document),
    byte_size: version.content.length,
    created_at: version.created_at ?? 'now',
  };
}

function mockUnifiedDiff(base: string, against: string) {
  const baseLines = base.trimEnd().split('\n');
  const againstLines = against.trimEnd().split('\n');
  return ['--- base', '+++ against', ...baseLines.map((line) => `-${line}`), ...againstLines.map((line) => `+${line}`)].join('\n');
}

function conflictRecord(conflict: MockConflict, resolved = false) {
  return {
    id: conflict.id,
    library_id: 'lib-notes',
    path: conflict.path,
    conflict_path: conflict.conflict_path ?? null,
    ours_version_id: conflict.ours_version_id ?? null,
    theirs_version_id: conflict.theirs_version_id ?? null,
    status: resolved ? 'resolved' : 'open',
    discovered_at: '2026-05-29T12:00:00Z',
    resolved_at: resolved ? '2026-05-29T12:05:00Z' : null,
  };
}

function link(overrides: MockLink) {
  return {
    src_doc_id: overrides.src_doc_id ?? 'doc-source',
    src_version_id: overrides.src_version_id ?? 'v-source',
    src_path: overrides.src_path ?? 'source.md',
    target_kind: overrides.target_kind ?? 'wiki_link',
    target_text: overrides.target_text ?? 'Target',
    target_doc_id: overrides.target_doc_id ?? null,
    target_path: overrides.target_path ?? null,
    target_anchor: overrides.target_anchor ?? null,
    alias: overrides.alias ?? null,
    start_offset: overrides.start_offset ?? 0,
    end_offset: overrides.end_offset ?? 8,
    resolved: overrides.resolved ?? true,
    resolution_status: overrides.resolution_status ?? 'resolved',
  };
}

function writeOutcome(document: MockDocument) {
  return {
    document: documentStub(document),
    transaction: {
      actor: null,
      committed_at: 'now',
      created_at: 'now',
      id: `tx-${document.version}`,
      library_id: 'lib-notes',
      message: null,
      provenance: {},
      source: 'rest',
      state: 'committed',
    },
    version: {
      byte_size: documentByteSize(document),
      content_hash: document.contentHash ?? null,
      content_type: documentContentType(document),
      created_at: 'now',
      document_id: document.id,
      id: document.version,
      inline_content: null,
      metadata: document.metadata ?? {},
      transaction_actor: null,
      transaction_message: null,
      transaction_provenance: {},
      transaction_source: 'rest',
      tx_id: `tx-${document.version}`,
    },
  };
}

function documentContentType(document: MockDocument) {
  return document.contentType ?? 'text/markdown';
}

function documentByteSize(document: MockDocument) {
  return document.byteSize ?? document.content.length;
}

async function notFound(route: Route) {
  await route.fulfill({ body: 'not found', status: 404 });
}
