import {
  BaseTableCellHeaderPlugin,
  BaseTableCellPlugin,
  BaseTablePlugin as PlateBaseTablePlugin,
  BaseTableRowPlugin,
  insertTable,
} from '@platejs/table';
import {
  defaultRules,
  type DeserializeMdOptions,
  type MdDecoration,
  type MdRules,
  type MdTable,
  type SerializeMdOptions,
} from '@platejs/markdown';
import {
  ElementApi,
  KEYS,
  type Descendant,
  type Path,
  type SlateEditor,
  type TElement,
  type TTableCellElement,
  type TTableElement,
  type TTableRowElement,
} from 'platejs';

// A GFM table round-trips for free once @platejs/table is registered — the only
// thing PlateJS's default markdown rule drops is per-column alignment. We store
// alignment as `align: TableAlign[]` on the table node (mirroring mdast exactly)
// and thread it through a thin wrapper around the default rule, which keeps the
// non-trivial cell-grouping / `<br/>`-join logic intact.

export {
  BaseTableCellHeaderPlugin,
  BaseTableCellPlugin,
  BaseTableRowPlugin,
};

export type TableAlign = 'left' | 'center' | 'right' | null;
export type TableElementWithShape = TTableElementWithAlign & {
  children: Array<TTableRowElement & { children: TTableCellElement[] }>;
  colSizes?: number[];
};
export type PlateValueWithTables = TableElementWithShape[];

export const BaseTablePlugin = PlateBaseTablePlugin.configure({
  options: { disableMerge: true },
});

// The Plate table node carries an optional per-column alignment array. `align`
// is optional, so a plain TTableElement is still assignable here — that keeps the
// serialize wrapper below assignable to PlateJS's stricter rule signature.
export type TTableElementWithAlign = TTableElement & { align?: TableAlign[] };

/** Validate one mdast align cell into our TableAlign (anything else → undefined). */
export function toAlign(value: unknown): 'left' | 'center' | 'right' | undefined {
  return value === 'left' || value === 'center' || value === 'right' ? value : undefined;
}

/** The column alignment for a cell at `colIndex`, read off the table node. */
export function columnAlignOf(tableNode: TElement, colIndex: number): TableAlign | undefined {
  const align = 'align' in tableNode ? tableNode.align : undefined;
  return Array.isArray(align) ? toAlign(align[colIndex]) : undefined;
}

export function normalizeTablesInValue<T extends Descendant[]>(value: T): T {
  return value.map((node) => normalizeTablesInNode(node)) as T;
}

function normalizeTablesInNode(node: Descendant): Descendant {
  if (!ElementApi.isElement(node)) return node;
  if (node.type === KEYS.table) return normalizeTableElement(node as TTableElementWithAlign);
  return {
    ...node,
    children: normalizeTablesInValue(node.children),
  };
}

function normalizeTableElement(tableNode: TTableElementWithAlign): TableElementWithShape {
  const rows = tableNode.children.filter(isTableRowElement);
  const width = rows.reduce((max, row) => Math.max(max, row.children.length), 0);
  const normalizedRows = rows.map((row, rowIndex) => normalizeTableRow(row, rowIndex, width));
  const { align: rawAlign, children: _children, colSizes, ...rest } = tableNode as TTableElementWithAlign & {
    colSizes?: unknown;
  };
  const normalized: Record<string, unknown> = {
    ...rest,
    type: KEYS.table,
    children: normalizedRows,
  };
  if (Array.isArray(rawAlign)) normalized.align = normalizedAlign(rawAlign, width);
  if (Array.isArray(colSizes) && colSizes.length === width) normalized.colSizes = colSizes;
  return normalized as TableElementWithShape;
}

function normalizeTableRow(
  rowNode: TTableRowElement,
  rowIndex: number,
  width: number
): TTableRowElement & { children: TTableCellElement[] } {
  const cellType = rowIndex === 0 ? KEYS.th : KEYS.td;
  const cells = rowNode.children.map((cellNode) => normalizeTableCell(cellNode, cellType));
  while (cells.length < width) cells.push(emptyTableCell(cellType));
  return { ...rowNode, type: KEYS.tr, children: cells };
}

function normalizeTableCell(cellNode: Descendant, cellType: string): TTableCellElement {
  if (!ElementApi.isElement(cellNode)) return emptyTableCell(cellType);
  const { children, colSpan: _colSpan, rowSpan: _rowSpan, type: _type, ...rest } = cellNode as TElement & {
    colSpan?: unknown;
    rowSpan?: unknown;
  };
  return {
    ...rest,
    type: cellType,
    children: normalizeTablesInValue(children),
  } as TTableCellElement;
}

export function emptyTableCell(type: string): TTableCellElement {
  return {
    type,
    children: [{ type: KEYS.p, children: [{ text: '' }] }],
  } as TTableCellElement;
}

function normalizedAlign(align: unknown[], width: number): TableAlign[] {
  return Array.from({ length: width }, (_unused, index) => toAlign(align[index]) ?? null);
}

function isTableRowElement(node: Descendant): node is TTableRowElement {
  return ElementApi.isElement(node) && node.type === KEYS.tr;
}

function isTableCellElement(node: Descendant): node is TTableCellElement {
  return ElementApi.isElement(node) && (node.type === KEYS.td || node.type === KEYS.th);
}

export function normalizeTableEntry(
  editor: SlateEditor,
  [node, path]: [TElement, Path]
): boolean {
  if (!ElementApi.isElement(node) || node.type !== editor.getType(KEYS.table)) return false;

  const table = node as TTableElementWithAlign & { colSizes?: unknown };
  const rows = table.children.filter(isTableRowElement);
  const width = rows.reduce((max, row) => Math.max(max, row.children.length), 0);
  if (Array.isArray(table.align)) {
    const nextAlign = normalizedAlign(table.align, width);
    if (!sameAlign(table.align, nextAlign)) {
      editor.tf.setNodes<TTableElementWithAlign>({ align: nextAlign }, { at: path });
      return true;
    }
  } else if ('align' in table) {
    editor.tf.unsetNodes('align', { at: path });
    return true;
  }

  if ('colSizes' in table && (!Array.isArray(table.colSizes) || table.colSizes.length !== width)) {
    editor.tf.unsetNodes('colSizes', { at: path });
    return true;
  }

  const rowType = editor.getType(KEYS.tr);
  const headerType = editor.getType(KEYS.th);
  const bodyType = editor.getType(KEYS.td);
  for (let rowIndex = 0; rowIndex < table.children.length; rowIndex += 1) {
    const row = table.children[rowIndex];
    if (!ElementApi.isElement(row) || row.type !== rowType) continue;
    if (row.children.length < width) {
      editor.tf.insertNodes(emptyTableCell(rowIndex === 0 ? headerType : bodyType), {
        at: [...path, rowIndex, row.children.length],
      });
      return true;
    }
    for (let colIndex = 0; colIndex < row.children.length; colIndex += 1) {
      const cell = row.children[colIndex];
      if (!isTableCellElement(cell)) continue;
      const cellPath = [...path, rowIndex, colIndex];
      const expectedType = rowIndex === 0 ? headerType : bodyType;
      if (cell.type !== expectedType) {
        editor.tf.setNodes<TTableCellElement>({ type: expectedType }, { at: cellPath });
        return true;
      }
      if ('colSpan' in cell || 'rowSpan' in cell) {
        editor.tf.unsetNodes(['colSpan', 'rowSpan'], { at: cellPath });
        return true;
      }
    }
  }

  return false;
}

function sameAlign(left: unknown[], right: TableAlign[]): boolean {
  return left.length === right.length && left.every((value, index) => value === right[index]);
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
