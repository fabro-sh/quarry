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
import { KEYS, type SlateEditor, type TElement, type TTableElement } from 'platejs';

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
export function turnIntoTable(editor: SlateEditor): void {
  // No nested tables: the normalizer unwraps them, so skip when already in one.
  if (editor.api.some({ match: { type: editor.getType(KEYS.table) } })) return;
  insertTable(editor, { colCount: 3, header: true, rowCount: 3 }, { select: true });
}
