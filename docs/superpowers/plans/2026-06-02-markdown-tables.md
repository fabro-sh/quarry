# Markdown-compatible tables — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add GFM table support to quarry's Plate editor — tables round-trip through the `.md` file, support inline marks in cells, insert/delete rows & columns, Tab navigation, editable column alignment, and (ephemeral) column resize.

**Architecture:** Mirror the existing block-feature pattern (Mermaid/Image/WikiLink). A `table.ts` codec module re-exports `@platejs/table`'s base plugins and defines `tableMdRules` (the only custom markdown logic — wraps PlateJS's default table rule to thread per-column `align`). A `table-element.tsx` React module supplies quarry-styled components via the `@platejs/table/react` plugins/hooks. The plugins + rules register across all three editor configs (live, codec, review). A trailing-block paragraph after a table already works (prior fix).

**Tech Stack:** PlateJS 52.x (`@platejs/table`, `@platejs/resizable`, `@platejs/markdown`), `remark-gfm` (already installed), React, bun, Playwright + Vitest.

**Spec:** `docs/superpowers/specs/2026-06-02-markdown-tables-design.md`

**Standards (hard rules):** bun (`bunx`/`bun run`, never pnpm); ESM; TypeScript strict — **no `any`, no `as` assertions, no non-null `!`** in production code (`as const` allowed); no circular deps; no loops in tests; commit only the listed files; commit messages end with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## Task 1: Dependencies + codec wiring + alignment round-trip

Build the durable core: install the table packages, register the base table plugins in the codec, and define `tableMdRules` so GFM tables (with alignment) round-trip through `markdownToPlateValue`/`plateValueToMarkdown`.

**Files:**
- Modify: `ui/package.json` (add `@platejs/table`, `@platejs/resizable`)
- Create: `ui/src/features/editor/table.ts`
- Modify: `ui/src/features/editor/markdown-codec.ts`
- Modify: `ui/src/features/review/review-md-rules.ts`
- Test: `ui/src/features/editor/markdown-codec.test.ts`

- [ ] **Step 1: Install the two table packages at the 52.x line**

Run (from `ui/`):
```bash
cd /Users/bhelmkamp/p/fabro-sh/quarry/ui
bun add @platejs/table@52.0.11 @platejs/resizable@52.0.11
```
Expected: both added to `package.json` dependencies, `bun.lock` updated. (Both are long-published; matches the other `@platejs/*` at `52.0.11`.) If `52.0.11` is unavailable, use the latest published `52.0.x` that resolves against `platejs@52.0.17`.

- [ ] **Step 2: Write the failing codec test (structure + alignment round-trip)**

Add to `ui/src/features/editor/markdown-codec.test.ts` (inside the existing `describe('markdown codec', …)`):
```ts
  it('round-trips a GFM table with inline marks in cells', () => {
    const md = '| Name | Role |\n| --- | --- |\n| **Ana** | `dev` |\n';
    const value = markdownToPlateValue(md);
    expect((value[0] as { type?: string }).type).toBe('table');
    const out = plateValueToMarkdown(value);
    expect(out).toContain('| Name');
    expect(out).toContain('**Ana**');
    expect(out).toContain('`dev`');
    // Idempotent: serializing the re-parsed output is stable.
    expect(plateValueToMarkdown(markdownToPlateValue(out))).toBe(out);
  });

  it('round-trips GFM column alignment (left/center/right)', () => {
    const md = '| L | C | R |\n| :-- | :-: | --: |\n| 1 | 2 | 3 |\n';
    const value = markdownToPlateValue(md);
    expect((value[0] as { align?: unknown }).align).toEqual(['left', 'center', 'right']);
    const out = plateValueToMarkdown(value);
    expect(out).toContain(':--');
    expect(out).toContain(':-:');
    expect(out).toContain('--:');
  });
```

- [ ] **Step 3: Run the test to verify it fails**

Run:
```bash
bunx vitest run src/features/editor/markdown-codec.test.ts -t "GFM table"
```
Expected: FAIL — without a table plugin registered, `remark-gfm`'s table mdast is dropped, so `value[0].type` is `undefined`/not `'table'`.

- [ ] **Step 4: Create `ui/src/features/editor/table.ts`**

```ts
import { insertTable } from '@platejs/table';
import {
  BaseTableCellHeaderPlugin,
  BaseTableCellPlugin,
  BaseTablePlugin,
  BaseTableRowPlugin,
} from '@platejs/table';
import {
  defaultRules,
  type DeserializeMdOptions,
  type MdDecoration,
  type MdRules,
  type MdTable,
  type SerializeMdOptions,
} from '@platejs/markdown';
import { KEYS, type PlateEditor, type TElement, type TTableElement } from 'platejs';

// A GFM table round-trips for free once @platejs/table is registered — the only
// thing PlateJS's default markdown rule drops is per-column alignment. We store
// alignment as `align: TableAlign[]` on the table node (mirroring mdast exactly)
// and thread it through a thin wrapper around the default rule, which keeps the
// non-trivial cell-grouping / `<br/>`-join logic intact.

export {
  BaseTableCellHeaderPlugin,
  BaseTableCellPlugin,
  BaseTablePlugin,
  BaseTableRowPlugin,
};

export type TableAlign = 'left' | 'center' | 'right' | null;

// The Plate table node carries an optional per-column alignment array. `align`
// is optional, so a plain TTableElement is still assignable here — that keeps the
// serialize wrapper below assignable to PlateJS's stricter rule signature.
export type TTableElementWithAlign = TTableElement & { align?: TableAlign[] };

/** Validate one mdast align cell into our TableAlign (anything else → undefined). */
export function toAlign(value: unknown): TableAlign | undefined {
  return value === 'left' || value === 'center' || value === 'right' ? value : undefined;
}

/** The column alignment for a cell at `colIndex`, read off the table node. */
export function columnAlignOf(tableNode: TElement, colIndex: number): TableAlign | undefined {
  const align = 'align' in tableNode ? tableNode.align : undefined;
  return Array.isArray(align) ? toAlign(align[colIndex]) : undefined;
}

const baseTableDeserialize = defaultRules.table?.deserialize;
const baseTableSerialize = defaultRules.table?.serialize;
if (!baseTableDeserialize || !baseTableSerialize) {
  throw new Error('Expected @platejs/markdown to provide default table rules');
}

// Wrap the default rule to carry `align` in both directions. mdast's table.align
// is read on load; our table node's align is written back on save (mdast-util-to-
// markdown emits the `:--:` delimiter row from it). colSizes is never threaded, so
// resize is dropped on save.
export const tableMdRules: MdRules = {
  table: {
    deserialize: (node: MdTable, deco: MdDecoration, options: DeserializeMdOptions) => ({
      ...baseTableDeserialize(node, deco, options),
      align: node.align,
    }),
    serialize: (node: TTableElementWithAlign, options: SerializeMdOptions) => ({
      ...baseTableSerialize(node, options),
      align: node.align,
    }),
  },
};

/** Insert a 3×3 starter table (header row + 2 body rows), cursor in the first cell. */
export function turnIntoTable(editor: PlateEditor): void {
  // No nested tables: the normalizer unwraps them, so skip when already in one.
  if (editor.api.some({ match: { type: editor.getType(KEYS.table) } })) return;
  insertTable(editor, { colCount: 3, header: true, rowCount: 3 }, { select: true });
}
```

- [ ] **Step 5: Register base table plugins + rules in the codec (`markdown-codec.ts`)**

In `ui/src/features/editor/markdown-codec.ts`:

Add the import near the other feature imports (after the `mermaid` import line):
```ts
import {
  BaseTableCellHeaderPlugin,
  BaseTableCellPlugin,
  BaseTablePlugin,
  BaseTableRowPlugin,
  tableMdRules,
} from './table';
```

Add the four base plugins to `baseMarkdownPlugins` (after `BaseMermaidPlugin`):
```ts
  BaseMermaidPlugin,
  BaseTablePlugin,
  BaseTableRowPlugin,
  BaseTableCellPlugin,
  BaseTableCellHeaderPlugin,
];
```

Spread `tableMdRules` into the codec `editor()` rules:
```ts
          rules: { ...wikiLinkMdRules, ...mermaidMdRules, ...tableMdRules },
```

- [ ] **Step 6: Spread `tableMdRules` into the review rules (`review-md-rules.ts`)**

In `ui/src/features/review/review-md-rules.ts`, add the import:
```ts
import { tableMdRules } from '../editor/table';
```
and spread it into the object returned by `reviewMdRules`:
```ts
  return {
    ...wikiLinkMdRules,
    ...mermaidMdRules,
    ...tableMdRules,
    suggestion: {
```

- [ ] **Step 7: Run the codec test to verify it passes**

Run:
```bash
bunx vitest run src/features/editor/markdown-codec.test.ts -t "GFM table"
```
Expected: PASS (both new tests).

- [ ] **Step 8: Run the full unit suite + typecheck**

Run:
```bash
bunx vitest run && bun run typecheck
```
Expected: all unit tests pass; `tsc -b` reports no errors. (No `any`/`as`/`!` introduced.)

- [ ] **Step 9: Commit**

```bash
git add ui/package.json ui/bun.lock ui/src/features/editor/table.ts ui/src/features/editor/markdown-codec.ts ui/src/features/review/review-md-rules.ts ui/src/features/editor/markdown-codec.test.ts
git commit -m "feat(editor): round-trip GFM tables through the markdown codec

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Live editor — render tables, insert via "Turn into"

Register the React table plugins and components in the live editor and wire "Table" into both "Turn into" menus. Cells render and are editable; typing autosaves.

**Files:**
- Create: `ui/src/features/editor/table-element.tsx`
- Modify: `ui/src/features/editor/PlateMarkdownEditor.tsx`
- Test: `ui/tests/workspace.spec.ts`

- [ ] **Step 1: Write the failing e2e tests**

Add to `ui/tests/workspace.spec.ts` (inside `test.describe('Quarry Browser smoke flows', …)`, after the mermaid tests):
```ts
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
    // 3 columns × 3 rows: 3 header cells + 6 body cells.
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
    await expect.poll(() => api.saveHeaders().length).toBeGreaterThan(0);
  });
```

- [ ] **Step 2: Run the e2e tests to verify they fail**

Run:
```bash
bunx playwright test workspace -g "table" --reporter=line
```
Expected: FAIL — no `<table>` rendered (no React table plugin), no "Table" menu item.

- [ ] **Step 3: Create `ui/src/features/editor/table-element.tsx`**

Adapted from Potion's `table-node.tsx`, restyled to quarry tokens, with `@platejs/selection` block-selection and all `as` casts removed. Resize handles and the alignment control are added in later tasks.

```tsx
import {
  TableCellHeaderPlugin,
  TableCellPlugin,
  TablePlugin,
  TableProvider,
  TableRowPlugin,
  useTableCellElement,
  useTableElement,
} from '@platejs/table/react';
import { type TTableCellElement } from 'platejs';
import {
  PlateElement,
  type PlateElementProps,
  useEditorPlugin,
  useReadOnly,
  withHOC,
} from 'platejs/react';
import { Plus } from 'lucide-react';

import { cn } from '../../lib/utils';

export const TableElement = withHOC(TableProvider, function TableElement(props: PlateElementProps) {
  const { editor, element } = props;
  const { tf } = useEditorPlugin(TablePlugin);
  const readOnly = useReadOnly();
  const { marginLeft, props: tableProps } = useTableElement();
  return (
    <PlateElement {...props} className="overflow-x-auto py-2" style={{ paddingLeft: marginLeft }}>
      <div className="group/table relative w-fit">
        <table className="my-0 table table-fixed border-collapse text-sm" {...tableProps}>
          <tbody className="min-w-full">{props.children}</tbody>
        </table>
        {readOnly ? null : (
          <>
            <button
              aria-label="Add row"
              className="absolute inset-x-0 -bottom-3 flex h-3 items-center justify-center rounded-sm bg-well text-muted opacity-0 transition-opacity hover:bg-line hover:text-body group-hover/table:opacity-100"
              onClick={() => tf.insert.tableRow({ at: editor.api.findPath(element) })}
              onMouseDown={(event) => event.preventDefault()}
              type="button"
            >
              <Plus size={12} />
            </button>
            <button
              aria-label="Add column"
              className="absolute inset-y-0 -right-3 flex w-3 items-center justify-center rounded-sm bg-well text-muted opacity-0 transition-opacity hover:bg-line hover:text-body group-hover/table:opacity-100"
              onClick={() => tf.insert.tableColumn({ at: editor.api.findPath(element) })}
              onMouseDown={(event) => event.preventDefault()}
              type="button"
            >
              <Plus size={12} />
            </button>
          </>
        )}
      </div>
    </PlateElement>
  );
});

export function TableRowElement(props: PlateElementProps) {
  return <PlateElement {...props} as="tr" />;
}

export function TableCellElement({
  isHeader,
  ...props
}: PlateElementProps<TTableCellElement> & { isHeader?: boolean }) {
  const { api } = useEditorPlugin(TablePlugin);
  const element = props.element;
  const { width } = useTableCellElement();
  return (
    <PlateElement
      {...props}
      as={isHeader ? 'th' : 'td'}
      attributes={{
        ...props.attributes,
        colSpan: api.table.getColSpan(element),
        rowSpan: api.table.getRowSpan(element),
      }}
      className={cn(
        'relative border border-line align-top',
        isHeader ? 'bg-well font-semibold text-ink' : 'text-body'
      )}
      style={{ maxWidth: width || 320, minWidth: width || 96 }}
    >
      <div className="px-3 py-1.5">{props.children}</div>
    </PlateElement>
  );
}

export function TableCellHeaderElement(props: PlateElementProps<TTableCellElement>) {
  return <TableCellElement {...props} isHeader />;
}

export const TableKit = [
  TablePlugin.withComponent(TableElement),
  TableRowPlugin.withComponent(TableRowElement),
  TableCellPlugin.withComponent(TableCellElement),
  TableCellHeaderPlugin.withComponent(TableCellHeaderElement),
];
```

- [ ] **Step 4: Register `TableKit` + rules + "Turn into" in `PlateMarkdownEditor.tsx`**

Add imports (near the `MermaidPlugin` import):
```ts
import { tableMdRules, turnIntoTable } from './table';
import { TableKit } from './table-element';
```
Add `Table` to the lucide import block:
```ts
  Table,
```
Add `...TableKit` to `plateMarkdownPlugins` (after `MermaidPlugin`):
```ts
  MermaidPlugin,
  ...TableKit,
```
Spread `tableMdRules` into the live `MarkdownPlugin` rules:
```ts
    options: { remarkPlugins: [remarkGfm, remarkInlineMarks], rules: { ...wikiLinkMdRules, ...mermaidMdRules, ...tableMdRules } },
```
Add "Table" to `TURN_INTO_ITEMS` (after the Mermaid entry):
```ts
  { icon: Workflow, label: 'Mermaid', value: 'mermaid' },
  { icon: Table, label: 'Table', value: 'table' },
];
```
Special-case it in the `TURN_INTO_ITEMS` dropdown `onSelect` (alongside the mermaid branch):
```ts
              onSelect={() => {
                if (item.value === 'mermaid') turnIntoMermaid(editor);
                else if (item.value === 'table') turnIntoTable(editor);
                else applyBlockType(editor, item.value);
                editor.tf.focus();
              }}
```
Add "Table" to `BLOCK_TURN_INTO` (after the Mermaid entry):
```ts
  { icon: Workflow, label: 'Mermaid diagram', apply: (editor) => turnIntoMermaid(editor) },
  { icon: Table, label: 'Table', apply: (editor) => turnIntoTable(editor) },
];
```

- [ ] **Step 5: Run the e2e tests to verify they pass**

Run:
```bash
bunx playwright test workspace -g "table" --reporter=line
```
Expected: PASS (render, turn-into, autosave). If the autosave test is flaky on timing, it polls `saveHeaders()` so it should settle; re-run once to confirm determinism.

- [ ] **Step 6: Typecheck**

Run:
```bash
bun run typecheck
```
Expected: no errors.

- [ ] **Step 7: Commit**

```bash
git add ui/src/features/editor/table-element.tsx ui/src/features/editor/PlateMarkdownEditor.tsx ui/tests/workspace.spec.ts
git commit -m "feat(editor): render GFM tables and insert via Turn into

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Column alignment (render + editable) and row/column delete

Render each cell's `text-align` from its column's alignment, and add a header-cell dropdown to set alignment and insert/delete columns, plus delete-row. Alignment persists to markdown.

**Files:**
- Modify: `ui/src/features/editor/table-element.tsx`
- Test: `ui/tests/workspace.spec.ts`

- [ ] **Step 1: Write the failing e2e test (alignment persists)**

Add to `ui/tests/workspace.spec.ts`:
```ts
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
    await expect.poll(() => api.lastSavedBody('t4.md')).toContain(':-:');
  });
```
> Note: `installMockApi` must expose `lastSavedBody(path)` returning the most recent PUT body for that document. If it does not already, add a small helper to the mock (mirroring `saveHeaders`) that records the request body per path. Check `ui/tests/workspace.spec.ts`'s `installMockApi` definition and extend it with `lastSavedBody`.

- [ ] **Step 2: Run it to verify it fails**

Run:
```bash
bunx playwright test workspace -g "column alignment persists" --reporter=line
```
Expected: FAIL — no "Column options" button exists yet.

- [ ] **Step 3: Add alignment rendering + the column dropdown to `table-element.tsx`**

Replace the import block and the `TableCellElement` / `TableCellHeaderElement` functions with the version below (everything else in the file stays).

New/updated imports at the top of `table-element.tsx`:
```tsx
import {
  TableCellHeaderPlugin,
  TableCellPlugin,
  TablePlugin,
  TableProvider,
  TableRowPlugin,
  useTableCellElement,
  useTableElement,
} from '@platejs/table/react';
import * as DropdownMenu from '@radix-ui/react-dropdown-menu';
import { AlignCenter, AlignLeft, AlignRight, ChevronDown, Plus, Trash2 } from 'lucide-react';
import { type TTableCellElement } from 'platejs';
import {
  PlateElement,
  type PlateElementProps,
  useEditorPlugin,
  useEditorRef,
  useEditorSelector,
  useReadOnly,
  withHOC,
} from 'platejs/react';

import { cn } from '../../lib/utils';
import { columnAlignOf, type TableAlign, type TTableElementWithAlign } from './table';
```

Updated cell components:
```tsx
const ALIGN_CLASS: Record<'left' | 'center' | 'right', string> = {
  center: 'text-center',
  left: 'text-left',
  right: 'text-right',
};

export function TableCellElement({
  isHeader,
  ...props
}: PlateElementProps<TTableCellElement> & { isHeader?: boolean }) {
  const { api } = useEditorPlugin(TablePlugin);
  const editor = useEditorRef();
  const element = props.element;
  const { colIndex, width } = useTableCellElement();
  // Reactively read this column's alignment off the parent table node so cells
  // restyle when alignment changes.
  const align = useEditorSelector((ed) => {
    const path = ed.api.findPath(element);
    if (!path || path.length < 3) return undefined;
    const tableEntry = ed.api.node<TTableElementWithAlign>({ at: path.slice(0, -2) });
    return tableEntry ? columnAlignOf(tableEntry[0], colIndex) : undefined;
  }, [element, colIndex]);
  return (
    <PlateElement
      {...props}
      as={isHeader ? 'th' : 'td'}
      attributes={{
        ...props.attributes,
        colSpan: api.table.getColSpan(element),
        rowSpan: api.table.getRowSpan(element),
      }}
      className={cn(
        'group/cell relative border border-line align-top',
        isHeader ? 'bg-well font-semibold text-ink' : 'text-body',
        align ? ALIGN_CLASS[align] : null
      )}
      style={{ maxWidth: width || 320, minWidth: width || 96 }}
    >
      <div className="px-3 py-1.5">{props.children}</div>
      {isHeader ? <ColumnMenu colIndex={colIndex} editor={editor} element={element} /> : null}
    </PlateElement>
  );
}

export function TableCellHeaderElement(props: PlateElementProps<TTableCellElement>) {
  return <TableCellElement {...props} isHeader />;
}
```

Add the `ColumnMenu` component (place it above `TableKit`):
```tsx
const menuItem =
  'flex w-full cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-sm text-body outline-none select-none data-highlighted:bg-well';

// Set `align[colIndex]` on the table node (immutably), driving both the rendered
// text-align and the serialized `:--:` delimiter.
function setColumnAlign(
  editor: ReturnType<typeof useEditorRef>,
  element: TTableCellElement,
  colIndex: number,
  value: TableAlign
): void {
  const path = editor.api.findPath(element);
  if (!path || path.length < 3) return;
  const tablePath = path.slice(0, -2);
  const tableEntry = editor.api.node<TTableElementWithAlign>({ at: tablePath });
  if (!tableEntry) return;
  const tableNode = tableEntry[0];
  const current = tableNode.align ?? [];
  // First row's cell count = column count (no merged cells in v1).
  const colCount = tableNode.children[0]?.children.length ?? colIndex + 1;
  const next: TableAlign[] = Array.from({ length: colCount }, (_unused, i) => {
    if (i === colIndex) return value;
    const existing = current[i];
    return existing === 'left' || existing === 'center' || existing === 'right' ? existing : null;
  });
  editor.tf.setNodes<TTableElementWithAlign>({ align: next }, { at: tablePath });
}

function ColumnMenu({
  colIndex,
  editor,
  element,
}: {
  colIndex: number;
  editor: ReturnType<typeof useEditorRef>;
  element: TTableCellElement;
}) {
  const { tf } = useEditorPlugin(TablePlugin);
  const readOnly = useReadOnly();
  if (readOnly) return null;
  // Select this header cell so selection-based transforms (delete column/row)
  // operate on this column.
  const focusColumn = () => {
    const path = editor.api.findPath(element);
    if (path) editor.tf.select(path);
  };
  return (
    <DropdownMenu.Root modal={false}>
      <DropdownMenu.Trigger asChild>
        <button
          aria-label="Column options"
          className="absolute right-0.5 top-0.5 inline-flex items-center rounded p-0.5 text-muted opacity-0 transition-opacity hover:text-body group-hover/cell:opacity-100 focus-visible:opacity-100"
          contentEditable={false}
          onMouseDown={(event) => event.preventDefault()}
          type="button"
        >
          <ChevronDown size={13} />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="end"
          className="z-50 min-w-44 rounded-md border border-line bg-raised p-1 shadow-lg"
          sideOffset={4}
        >
          <DropdownMenu.Item className={menuItem} onSelect={() => setColumnAlign(editor, element, colIndex, 'left')}>
            <AlignLeft className="shrink-0 text-muted" size={15} /> Align left
          </DropdownMenu.Item>
          <DropdownMenu.Item className={menuItem} onSelect={() => setColumnAlign(editor, element, colIndex, 'center')}>
            <AlignCenter className="shrink-0 text-muted" size={15} /> Align center
          </DropdownMenu.Item>
          <DropdownMenu.Item className={menuItem} onSelect={() => setColumnAlign(editor, element, colIndex, 'right')}>
            <AlignRight className="shrink-0 text-muted" size={15} /> Align right
          </DropdownMenu.Item>
          <div className="my-1 h-px bg-line" />
          <DropdownMenu.Item
            className={menuItem}
            onSelect={() => {
              const path = editor.api.findPath(element);
              if (path) tf.insert.tableColumn({ before: true, fromCell: path });
            }}
          >
            <Plus className="shrink-0 text-muted" size={15} /> Insert column left
          </DropdownMenu.Item>
          <DropdownMenu.Item
            className={menuItem}
            onSelect={() => {
              const path = editor.api.findPath(element);
              if (path) tf.insert.tableColumn({ fromCell: path });
            }}
          >
            <Plus className="shrink-0 text-muted" size={15} /> Insert column right
          </DropdownMenu.Item>
          <div className="my-1 h-px bg-line" />
          <DropdownMenu.Item
            className={cn(menuItem, 'text-danger')}
            onSelect={() => {
              focusColumn();
              tf.remove.tableColumn();
            }}
          >
            <Trash2 className="shrink-0 text-danger" size={15} /> Delete column
          </DropdownMenu.Item>
          <DropdownMenu.Item
            className={cn(menuItem, 'text-danger')}
            onSelect={() => {
              focusColumn();
              tf.remove.tableRow();
            }}
          >
            <Trash2 className="shrink-0 text-danger" size={15} /> Delete row
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
```

- [ ] **Step 4: Run the alignment e2e test to verify it passes**

Run:
```bash
bunx playwright test workspace -g "column alignment persists" --reporter=line
```
Expected: PASS — saved body contains `:-:`.

- [ ] **Step 5: Run all table e2e + typecheck**

Run:
```bash
bunx playwright test workspace -g "table|column" --reporter=line && bun run typecheck
```
Expected: all table/column tests pass; no type errors.

- [ ] **Step 6: Commit**

```bash
git add ui/src/features/editor/table-element.tsx ui/tests/workspace.spec.ts
git commit -m "feat(editor): editable, round-tripping table column alignment

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Ephemeral column resize

Add drag-to-resize handles that write `colSizes` (editor-only — never serialized, so resize doesn't dirty the document and resets on reload).

**Files:**
- Modify: `ui/src/features/editor/table-element.tsx`
- Test: `ui/tests/workspace.spec.ts`

- [ ] **Step 1: Write the failing e2e test (resize does not save)**

Add to `ui/tests/workspace.spec.ts`:
```ts
  test('resizing a table column does not trigger a save', async ({ page }) => {
    const api = await installMockApi(page, {
      documents: [
        { content: '# Doc\n\n| A | B |\n| --- | --- |\n| x | y |\n', id: 'doc-t5', metadata: { title: 'T5' }, path: 't5.md', version: 'v1' },
      ],
    });
    await page.goto('/');
    await page.getByRole('treeitem', { name: /T5/ }).click();
    const editor = page.getByLabel('Plate markdown editor');
    const handle = editor.getByTestId('column-resize').first();
    await expect(handle).toBeAttached();
    const box = (await handle.boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + 60, box.y + box.height / 2, { steps: 8 });
    await page.mouse.up();
    // Resize is UI-only — it must not produce a PUT.
    await page.waitForTimeout(800);
    expect(api.saveHeaders().length).toBe(0);
  });
```

- [ ] **Step 2: Run it to verify it fails**

Run:
```bash
bunx playwright test workspace -g "resizing a table column" --reporter=line
```
Expected: FAIL — no `column-resize` handle exists yet.

- [ ] **Step 3: Add resize handles to `TableCellElement`**

Add the resizable import, and add `useTableCellElementResizable` to the **existing** `@platejs/table/react` import (don't create a second import from that module):
```tsx
import { ResizeHandle } from '@platejs/resizable';
// in the existing @platejs/table/react import, add: useTableCellElementResizable
```
Inside `TableCellElement`, change the existing `useTableCellElement()` destructure to also pull `colSpan` and `rowIndex`, and add the resizable hook + `readOnly`:
```tsx
  const { colIndex, colSpan, rowIndex, width } = useTableCellElement();
  const readOnly = useReadOnly();
  const { bottomProps, hiddenLeft, leftProps, rightProps } = useTableCellElementResizable({
    colIndex,
    colSpan,
    rowIndex,
  });
```
(`TableCellElement` already calls `useReadOnly()` indirectly only inside `ColumnMenu`; add the top-level `readOnly` here for the resize guard. Do not call `useTableCellElement()` twice.)

Add the handles as the last children inside the `PlateElement` (after the content `div`, before the closing tag), guarded by `!readOnly`:
```tsx
      {readOnly ? null : (
        <div className="absolute inset-0 select-none" contentEditable={false}>
          <ResizeHandle {...rightProps} className="absolute -right-1 top-0 h-full w-2 cursor-col-resize" data-testid="column-resize" />
          <ResizeHandle {...bottomProps} className="absolute -bottom-1 left-0 h-2 w-full cursor-row-resize" />
          {hiddenLeft ? null : (
            <ResizeHandle {...leftProps} className="absolute -left-1 top-0 h-full w-2 cursor-col-resize" />
          )}
        </div>
      )}
```

- [ ] **Step 4: Run the resize e2e test to verify it passes**

Run:
```bash
bunx playwright test workspace -g "resizing a table column" --reporter=line
```
Expected: PASS — handle is attached, drag completes, zero PUTs.

- [ ] **Step 5: Add a codec guard test (colSizes never serialized)**

Add to `ui/src/features/editor/markdown-codec.test.ts`:
```ts
  it('drops table colSizes when serializing (resize is editor-only)', () => {
    const md = '| A | B |\n| --- | --- |\n| x | y |\n';
    const value = markdownToPlateValue(md);
    const withSizes = [{ ...(value[0] as Record<string, unknown>), colSizes: [200, 200] }, ...value.slice(1)];
    expect(plateValueToMarkdown(withSizes)).toBe(plateValueToMarkdown(value));
  });
```

- [ ] **Step 6: Run unit + full table e2e + typecheck**

Run:
```bash
bunx vitest run src/features/editor/markdown-codec.test.ts && bunx playwright test workspace -g "table|column|resizing" --reporter=line && bun run typecheck
```
Expected: all pass, no type errors.

- [ ] **Step 7: Commit**

```bash
git add ui/src/features/editor/table-element.tsx ui/tests/workspace.spec.ts ui/src/features/editor/markdown-codec.test.ts
git commit -m "feat(editor): ephemeral table column resize

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Full regression + review-path verification

Confirm the whole suite is green and that a table survives a review-mode (RFM) save, since `reviewMdRules` is the path the live editor actually serializes through.

**Files:**
- Test: `ui/src/features/review/` (add one rfm-codec round-trip test if an `rfm-codec.test.ts` exists; otherwise add to the nearest review codec test)

- [ ] **Step 1: Write a review-path round-trip test**

Locate the review codec test file (e.g. `ui/src/features/review/rfm-codec.test.ts`). Add:
```ts
  it('round-trips a GFM table through the review codec', () => {
    const md = '| A | B |\n| :-- | --: |\n| x | y |\n';
    const { value, meta } = markdownToReview(md);
    expect((value[0] as { type?: string }).type).toBe('table');
    const out = reviewToMarkdown(value, meta);
    expect(out).toContain(':--');
    expect(out).toContain('--:');
    expect(out).toContain('| x');
  });
```
> Import `markdownToReview`, `reviewToMarkdown` from `./rfm-codec` if not already imported. If no such test file exists, create `ui/src/features/review/rfm-codec.test.ts` with the standard `describe(...)` wrapper and the import.

- [ ] **Step 2: Run it**

Run:
```bash
bunx vitest run src/features/review
```
Expected: PASS (table + alignment survive the review save path).

- [ ] **Step 3: Full suite**

Run:
```bash
bunx vitest run && bun run typecheck && bunx playwright test --reporter=line
```
Expected: all unit tests pass, no type errors, all e2e pass. (If the unrelated "resolves a Git conflict using keyboard-only navigation" e2e flakes, re-run it in isolation to confirm it is not caused by this change.)

- [ ] **Step 4: Commit**

```bash
git add ui/src/features/review/rfm-codec.test.ts
git commit -m "test(editor): verify tables survive the review save path

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-review notes (for the implementer)

- **No `as` / `!` / `any`:** the only tricky spots are the markdown rule wrapper (handled by the `baseTableDeserialize`/`baseTableSerialize` guard + the optional `align` keeping contravariance valid) and reading `align`/`colSizes` off loosely-typed nodes (handled via `'align' in node` narrowing + the `toAlign(unknown)` validator). If TS still complains at a node-prop read, route it through a small `unknown`-typed validator rather than reaching for a cast.
- **`installMockApi` capabilities:** Task 2 uses `api.saveHeaders()` (already present — used by the autosave/mermaid tests). Task 3 needs `api.lastSavedBody(path)`; verify/extend the mock helper. Do not modify shared mutable state across tests — each test installs its own mock.
- **Three-config registration:** double-check `tableMdRules` is spread in all three (`markdown-codec.ts` `editor()`, live `PlateMarkdownEditor.tsx`, `review-md-rules.ts`) and the base plugins are in `baseMarkdownPlugins`. A table that loads but saves blank means a missing review-path rule.
- **Turn-into while inside a table** is a no-op by design (`turnIntoTable` guard).
- **Deviation from spec Section 3 (rows):** v1 ships row *append* (the table's bottom "+") and delete-row (in the column menu), but not insert-row-above/below. Adding those is a small follow-up (`tf.insert.tableRow({ before, fromRow })`) if wanted; left out to keep the v1 surface tight. Insert-column left/right *is* included.
