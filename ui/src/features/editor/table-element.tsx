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
