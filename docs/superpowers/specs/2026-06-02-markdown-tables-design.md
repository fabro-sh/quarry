# Markdown-compatible tables — design

**Date:** 2026-06-02
**Status:** Approved design, pre-implementation
**Repo area:** `ui/` (quarry-browser, PlateJS editor)

## Goal

Add GFM (GitHub Flavored Markdown) table support to the Plate editor. Tables must
round-trip through the `.md` file so they survive Git export and read cleanly in any
editor — quarry's `.md` stays the single source of truth. Today `remark-gfm` already
parses table syntax, but no table plugin is registered, so **tables in a loaded
document are silently dropped**. This feature makes them first-class.

## Locked decisions

| Decision | Choice | Rationale |
|---|---|---|
| **Fidelity** | Markdown-faithful only | Header + body rows, inline marks in cells, insert/delete rows & columns, Tab nav, GFM column alignment. No merged cells, borders, or backgrounds — none survive a save to GFM, so we don't offer them and avoid silent data loss. |
| **Column resize** | UI-only, ephemeral | Drag-to-resize works in-session but is never written to markdown (GFM has no width), so widths reset on reload. Accepted trade-off. |
| **Alignment** | Preserve **and** editable | GFM per-column alignment (`:---`, `:--:`, `---:`) is kept on round-trip and changeable via a per-column control. |
| **Insertion** | "Turn into" menu | Add "Table" to the existing floating-toolbar and block-handle "Turn into" menus (like Mermaid). Inserts a 3-column × 3-row starter (1 header row + 2 body rows). Quarry has no slash menu. |
| **Architecture** | A — mirror quarry's block-feature pattern | A `table.ts` + `table-element.tsx` pair using `@platejs/table` plugins/hooks, exactly like Mermaid/Image/WikiLink. |
| **Markdown rules** | Wrap `defaultRules.table` | Reuse PlateJS's cell-grouping deserialize/serialize; add only `align` threading. Default rules drop alignment, so this wrapper is required. |

## Reference implementations

- **PlateJS source** (`~/p/udecode/plate`, MIT): `@platejs/table` provides `BaseTablePlugin`, `BaseTableRowPlugin`, `BaseTableCellPlugin`, `BaseTableCellHeaderPlugin` (and `/react` variants `TablePlugin`, … with Tab/arrow `onKeyDownTable`, `insertTable`/`insertTableRow`/`insertTableColumn`/`deleteRow`/`deleteColumn` transforms, and `useTableCellElement`/`useTableCellElementResizable` hooks). `@platejs/markdown` exports `defaultRules` (with the `table`/`td`/`th`/`tr` rules) and the `MdTable` type. Default table rules build `th` for row 0, `td` otherwise, group cell children into paragraphs, and join multi-block cells with `<br/>` — **but ignore `align` in both directions**.
- **Potion** (`~/p/fabro-sh/potion`, MIT): production wiring — registers the four table plugins via a `TableKit`, ships `table-node.tsx` (cell components using `ResizeHandle` from `@platejs/resizable` + the resize hook, add-row/add-column hover buttons), uses `remark-gfm` for round-trip with **no custom table rules**. We reuse its component structure, restyled to quarry tokens, and add the alignment handling Potion lacks.

**Key divergence from Potion:** Potion persists Plate JSON + Yjs and treats markdown as export-only, so it never needs alignment fidelity. Quarry round-trips everything through markdown, so we add custom `align` rules and deliberately omit the non-GFM features (merge/borders/background/persisted widths) Potion exposes.

---

## Section 1 — Architecture & module boundaries

Mirror the established quarry pattern (a `Base*` plugin + an `apply*`/rules module in `editor/`, a React component plugin via `.withComponent()`, registered across the three editor configs). Tables need **no `apply*` post-processor** — unlike Mermaid (which rewrites code blocks) or WikiLink (which parses `[[..]]` text), the default deserialize already produces table nodes; we only enrich them with `align` inside the markdown rule.

**New files:**

- **`src/features/editor/table.ts`** — the pure/codec module:
  - Re-exports `BaseTablePlugin`, `BaseTableRowPlugin`, `BaseTableCellPlugin`, `BaseTableCellHeaderPlugin` from `@platejs/table`.
  - `TableAlign = 'left' | 'center' | 'right' | null` and the table node's `align?: TableAlign[]`.
  - `tableMdRules` — the only custom markdown logic (Section 2).
  - `turnIntoTable(editor)` — inserts the 3×3 starter (Section 4).
- **`src/features/editor/table-element.tsx`** — the live React layer:
  - `TableElement`, `TableRowElement`, `TableCellElement`, `TableCellHeaderElement`, styled with quarry tokens (`text-ink`, `border-line`, `bg-raised`, …) using `@platejs/table/react` hooks + `@platejs/resizable`'s `ResizeHandle`.
  - The per-column alignment + insert/delete control.
  - `TableKit = [TablePlugin.withComponent(TableElement), TableRowPlugin.withComponent(TableRowElement), TableCellPlugin.withComponent(TableCellElement), TableCellHeaderPlugin.withComponent(TableCellHeaderElement)]`.

**Dependencies:** add `@platejs/table` and `@platejs/resizable`, pinned to the installed `52.x` line (the other `@platejs/*` packages are `52.0.11`; `platejs` is `52.0.17`). Both are long-published, not new releases.

## Section 2 — Data model & markdown round-trip

**Node shapes:**

```
table : { type:'table', align?: TableAlign[], colSizes?: number[], children: tr[] }
tr    : { type:'tr',  children: (th|td)[] }
th|td : { type:'td'|'th', children: [p, …] }   // row 0 → th, else td
```

- `align` is **per-column**, mirroring mdast's `table.align` exactly. Single source of truth; cells read their column's value for `text-align`.
- `colSizes` is **editor-only** — set by resize, never serialized.

**Load** (`markdown → value`):

```
remark-gfm  →  mdast table (with .align)
            →  defaultRules.table.deserialize  → tr / th / td (cell children grouped into paragraphs)
            →  our wrapper attaches  align: node.align
```

**Save** (`value → markdown`):

```
our table.serialize = { ...defaultRules.table.serialize(node), align: node.align }
            →  mdast table (with .align)  →  mdast-util-to-markdown emits the `:--:` delimiter row
colSizes : never emitted  →  resize is dropped
```

**`tableMdRules` (shape):**

```ts
export const tableMdRules = {
  table: {
    deserialize: (node, deco, options) => ({
      ...defaultRules.table.deserialize(node, deco, options),
      align: node.align,
    }),
    serialize: (node, options) => ({
      ...defaultRules.table.serialize(node, options),
      align: node.align,
    }),
  },
};
```

`td`/`th`/`tr` keep the defaults. Wrapping (not replacing) preserves the non-trivial
cell-grouping / `<br/>`-join logic.

**Two consequences that fall out for free:**

1. **Resize never dirties the document.** Changing `colSizes` re-serializes to byte-identical markdown → the editor's `onValueChange` equality guard (`nextMarkdown === lastSerializedRef.current`) short-circuits → no autosave. Asserted in tests.
2. **Always a line after a table.** The `TrailingBlockPlugin` added in the prior fix treats a table (non-paragraph) as the last block and appends a trailing editable paragraph; `stripTrailingEmptyParagraphs` removes it on save. So typing after a table already works, with no extra code.

## Section 3 — UI & interactions

- **Alignment rendering:** each cell applies `text-align` from `table.align[columnIndex]` (column index = the cell's position in its row).
- **Column header control:** a small dropdown on each header (`th`) cell — set alignment (◀ left / ● center / ▶ right), insert column left/right, delete column. Updates `table.align[col]` / runs the table transforms.
- **Row control:** a row affordance (left gutter) — insert row above/below, delete row.
- **Quick add:** hover "+" affordances on the table's right edge (add column) and bottom edge (add row), Potion-style.
- **Navigation:** Tab / Shift-Tab / arrow cell movement from `@platejs/table` (`onKeyDownTable`), no custom code.
- **Resize:** `useTableCellElementResizable` + `ResizeHandle`, writing `colSizes` (ephemeral).
- **Read-only (Viewing mode):** all controls/handles hidden; cells render but aren't editable.

## Section 4 — Insertion

`turnIntoTable(editor)` inserts a starter table via `@platejs/table`'s `insertTable`,
producing **3 columns × 3 rows** where row 0 is a header (`th`) row and rows 1–2 are
empty body (`td`) rows, cursor in the first cell. Wired into:

- `TURN_INTO_ITEMS` (floating-toolbar "Turn into" dropdown) — special-cased in `onSelect` like Mermaid.
- `BLOCK_TURN_INTO` (block drag-handle submenu) — `apply: (editor) => turnIntoTable(editor)`.

Guard: if the selection is already inside a table, "Table" is a no-op (no nested
tables; the normalizer would unwrap them anyway).

## Section 5 — Registration points

All three editor configs must register the table plugins, and all three markdown-rule
objects must include `tableMdRules`, or load/save will diverge:

| Location | Change |
|---|---|
| `markdown-codec.ts` › `baseMarkdownPlugins` | add the four `Base*` table plugins |
| `markdown-codec.ts` › `editor()` rules | spread `...tableMdRules` |
| `PlateMarkdownEditor.tsx` › `plateMarkdownPlugins` | add `...TableKit` |
| `PlateMarkdownEditor.tsx` › live `MarkdownPlugin` rules | spread `...tableMdRules` |
| `PlateMarkdownEditor.tsx` › `TURN_INTO_ITEMS` + `BLOCK_TURN_INTO` | add "Table" |
| `review-md-rules.ts` › `reviewMdRules` | spread `...tableMdRules` |

## Section 6 — Testing

- **Unit** (`src/features/editor/markdown-codec.test.ts`):
  - Round-trip a GFM table: structure (`table`/`tr`/`th`/`td`) + inline marks in cells (bold, link, `code`) → serialize back to the same fence.
  - Round-trip alignment: `| :--- | :--: | ---: |` preserved byte-for-byte; `align` lands on the table node.
- **E2E** (`tests/workspace.spec.ts`, mirroring the Mermaid tests + `installMockApi`):
  - A document with a GFM table renders editable cells.
  - "Turn into → Table" inserts a 3×3 table.
  - Typing in a cell autosaves (`saveHeaders` asserted).
  - Setting a column's alignment persists the `:--:` to saved markdown.
  - Resizing a column does **not** trigger a save (UI-only).

## Out of scope (v1)

- Merged cells (colSpan/rowSpan), cell borders, background colors — not representable in GFM.
- Persisted column widths — ephemeral by decision.
- Converting a table back to plain text via "Turn into" — use the block menu's Delete.
- Multi-cell-selection formatting — basic behavior only.
- Block-level content inside cells — GFM cells are inline-only (default rules join multi-block with `<br/>`); we keep cells single-paragraph in practice.

## Constraints

bun (not pnpm); ESM; TypeScript strict — no `any`, no `as` assertions, no non-null
`!` in production code (`as const` allowed); no circular dependencies. Commit only when
asked. The new module must register everywhere load **and** the review save path read
their rules, so a table survives both a plain edit and a review-mode save.
