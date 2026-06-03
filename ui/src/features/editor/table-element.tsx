import { ResizeHandle } from '@platejs/resizable';
import {
  TableCellHeaderPlugin,
  TableCellPlugin,
  TablePlugin,
  TableProvider,
  TableRowPlugin,
  useTableCellElement,
  useTableCellElementResizable,
  useTableElement,
} from '@platejs/table/react';
import * as DropdownMenu from '@radix-ui/react-dropdown-menu';
import { AlignCenter, AlignLeft, AlignRight, ChevronDown, Plus, Trash2 } from 'lucide-react';
import { KEYS, type Path, type TTableCellElement, type TTableRowElement } from 'platejs';
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
          <div contentEditable={false}>
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
          </div>
        )}
      </div>
    </PlateElement>
  );
});

export function TableRowElement(props: PlateElementProps) {
  return <PlateElement {...props} as="tr" />;
}

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
  const readOnly = useReadOnly();
  const element = props.element;
  const { colIndex, colSpan, rowIndex, width } = useTableCellElement();
  const { bottomProps, hiddenLeft, leftProps, rightProps } = useTableCellElementResizable({
    colIndex,
    colSpan,
    rowIndex,
  });
  // Reactively read this column's alignment off the parent table node so cells
  // restyle when alignment changes.
  const align = useEditorSelector((ed) => {
    const path = ed.api.findPath(element);
    if (!path || path.length < 3) return undefined;
    const tableEntry = ed.api.node<TTableElementWithAlign>(path.slice(0, -2));
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
      {readOnly ? null : (
        // Cell z-band: content/resize handles z-10–z-20, column-menu trigger z-30,
        // portalled menus z-50. The wrapper is pointer-events-none so only the thin
        // edge handles (pointer-events-auto) capture drags; clicks pass to the cell.
        <div className="pointer-events-none absolute inset-0 z-20 select-none" contentEditable={false}>
          <ResizeHandle
            {...rightProps}
            className="pointer-events-auto absolute -right-1 top-0 h-full w-2 cursor-col-resize"
            data-testid="column-resize"
          />
          <ResizeHandle {...bottomProps} className="pointer-events-auto absolute -bottom-1 left-0 h-2 w-full cursor-row-resize" />
          {hiddenLeft ? null : (
            <ResizeHandle {...leftProps} className="pointer-events-auto absolute -left-1 top-0 h-full w-2 cursor-col-resize" />
          )}
        </div>
      )}
    </PlateElement>
  );
}

export function TableCellHeaderElement(props: PlateElementProps<TTableCellElement>) {
  return <TableCellElement {...props} isHeader />;
}

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
  const tableEntry = editor.api.node<TTableElementWithAlign>(tablePath);
  if (!tableEntry) return;
  const tableNode = tableEntry[0];
  const current = tableNode.align ?? [];
  // First row's cell count = column count (no merged cells in v1). Query the row
  // as a typed element so `.children` is the cell array, not a Descendant union.
  const firstRow = editor.api.node<TTableRowElement>([...tablePath, 0]);
  const colCount = firstRow ? firstRow[0].children.length : colIndex + 1;
  const next: TableAlign[] = Array.from({ length: colCount }, (_unused, i) => {
    if (i === colIndex) return value;
    const existing = current[i];
    return existing === 'left' || existing === 'center' || existing === 'right' ? existing : null;
  });
  editor.tf.setNodes<TTableElementWithAlign>({ align: next }, { at: tablePath });
}

/** Read the table node's current `align` array, or undefined if unset. */
function columnAlignArray(
  editor: ReturnType<typeof useEditorRef>,
  tablePath: Path
): TableAlign[] | undefined {
  const tableEntry = editor.api.node<TTableElementWithAlign>(tablePath);
  return tableEntry ? tableEntry[0].align : undefined;
}

// The library's insert column transform rebuilds the table node and DROPS our
// `align` array entirely. So snapshot `align` BEFORE the transform, then splice a
// new column's slot in and write it back. No-op when no explicit alignment was
// set (prior align undefined).
function shiftColumnAlign(
  editor: ReturnType<typeof useEditorRef>,
  tablePath: Path,
  prior: TableAlign[] | undefined,
  at: number
): void {
  if (!prior) return;
  const next = [...prior];
  next.splice(at, 0, null);
  editor.tf.setNodes<TTableElementWithAlign>({ align: next }, { at: tablePath });
}

// Delete the column at colIndex by removing each row's cell at that index
// (selection-independent — the library's selection-based transform no-ops here
// because the dropdown clears the editor selection). Reindex `align` to match.
// Deleting the last remaining column drops the whole table.
function deleteColumnAt(
  editor: ReturnType<typeof useEditorRef>,
  element: TTableCellElement,
  colIndex: number
): void {
  const path = editor.api.findPath(element);
  if (!path || path.length < 3) return;
  const tablePath = path.slice(0, -2);
  const tableEntry = editor.api.node<TTableElementWithAlign>(tablePath);
  if (!tableEntry) return;
  const rowCount = tableEntry[0].children.length;
  const firstRow = editor.api.node<TTableRowElement>([...tablePath, 0]);
  const colCount = firstRow ? firstRow[0].children.length : 0;
  const prior = tableEntry[0].align;
  editor.tf.withoutNormalizing(() => {
    if (colCount <= 1) {
      editor.tf.removeNodes({ at: tablePath });
      return;
    }
    for (let r = rowCount - 1; r >= 0; r -= 1) {
      editor.tf.removeNodes({ at: [...tablePath, r, colIndex] });
    }
    if (prior) {
      const next = [...prior];
      next.splice(colIndex, 1);
      editor.tf.setNodes<TTableElementWithAlign>({ align: next }, { at: tablePath });
    }
  });
}

// Delete the row that this header cell belongs to. Deleting the last row drops
// the whole table.
function deleteRowAt(
  editor: ReturnType<typeof useEditorRef>,
  element: TTableCellElement
): void {
  const path = editor.api.findPath(element);
  if (!path || path.length < 3) return;
  const tablePath = path.slice(0, -2);
  const rowPath = path.slice(0, -1);
  const tableEntry = editor.api.node<TTableElementWithAlign>(tablePath);
  if (!tableEntry) return;
  if (tableEntry[0].children.length <= 1) {
    editor.tf.removeNodes({ at: tablePath });
    return;
  }
  editor.tf.withoutNormalizing(() => {
    editor.tf.removeNodes({ at: rowPath });
    // The menu is header-anchored, so we just deleted the header row — promote
    // the new first row to a real header so the table stays well-formed.
    const newFirstRow = editor.api.node<TTableRowElement>([...tablePath, 0]);
    if (newFirstRow) {
      const cellCount = newFirstRow[0].children.length;
      const thType = editor.getType(KEYS.th);
      for (let c = 0; c < cellCount; c += 1) {
        editor.tf.setNodes<TTableCellElement>({ type: thType }, { at: [...tablePath, 0, c] });
      }
    }
  });
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
  return (
    <DropdownMenu.Root modal={false}>
      <DropdownMenu.Trigger asChild>
        <button
          aria-label="Column options"
          // z-30 keeps this trigger above the z-20 resize overlay so it stays clickable.
          className="absolute right-0.5 top-0.5 z-30 inline-flex items-center rounded p-0.5 text-muted opacity-0 transition-opacity hover:text-body group-hover/cell:opacity-100 focus-visible:opacity-100"
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
              if (!path || path.length < 3) return;
              const tablePath = path.slice(0, -2);
              const prior = columnAlignArray(editor, tablePath);
              tf.insert.tableColumn({ before: true, fromCell: path });
              shiftColumnAlign(editor, tablePath, prior, colIndex);
            }}
          >
            <Plus className="shrink-0 text-muted" size={15} /> Insert column left
          </DropdownMenu.Item>
          <DropdownMenu.Item
            className={menuItem}
            onSelect={() => {
              const path = editor.api.findPath(element);
              if (!path || path.length < 3) return;
              const tablePath = path.slice(0, -2);
              const prior = columnAlignArray(editor, tablePath);
              tf.insert.tableColumn({ fromCell: path });
              shiftColumnAlign(editor, tablePath, prior, colIndex + 1);
            }}
          >
            <Plus className="shrink-0 text-muted" size={15} /> Insert column right
          </DropdownMenu.Item>
          <div className="my-1 h-px bg-line" />
          <DropdownMenu.Item
            className={cn(menuItem, 'text-danger')}
            onSelect={() => deleteColumnAt(editor, element, colIndex)}
          >
            <Trash2 className="shrink-0 text-danger" size={15} /> Delete column
          </DropdownMenu.Item>
          <DropdownMenu.Item
            className={cn(menuItem, 'text-danger')}
            onSelect={() => deleteRowAt(editor, element)}
          >
            <Trash2 className="shrink-0 text-danger" size={15} /> Delete row
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

export const TableKit = [
  TablePlugin.withComponent(TableElement),
  TableRowPlugin.withComponent(TableRowElement),
  TableCellPlugin.withComponent(TableCellElement),
  TableCellHeaderPlugin.withComponent(TableCellHeaderElement),
];
