import { describe, expect, it } from 'vitest';

import { normalizeTablesInValue, type PlateValueWithTables } from './table';

const text = (value: string) => ({ text: value });
const p = (value: string) => ({ type: 'p', children: [text(value)] });
const cell = (type: 'td' | 'th', value: string, attrs: Record<string, unknown> = {}) => ({
  type,
  ...attrs,
  children: [p(value)],
});
const row = (cells: Array<ReturnType<typeof cell>>) => ({ type: 'tr', children: cells });
const table = (rows: Array<ReturnType<typeof row>>, attrs: Record<string, unknown> = {}) => ({
  type: 'table',
  ...attrs,
  children: rows,
});

const rowWidths = (value: PlateValueWithTables) =>
  (value[0].children as Array<{ children: unknown[] }>).map((tableRow) => tableRow.children.length);

describe('table normalization', () => {
  it('preserves object identity when there are no tables to normalize', () => {
    const value = [p('plain text')];

    expect(normalizeTablesInValue(value)).toBe(value);
  });

  it('repairs ragged rows to the widest physical row without losing bottom-right content', () => {
    const normalized = normalizeTablesInValue([
      table([
        row([cell('th', 'A'), cell('th', 'B'), cell('th', 'C')]),
        row([cell('td', '1'), cell('td', '2'), cell('td', '3')]),
        row([cell('td', '4'), cell('td', '5'), cell('td', '6'), cell('td', 'keep')]),
      ]),
    ]);

    expect(rowWidths(normalized)).toEqual([4, 4, 4]);
    expect(normalized[0].children[2].children[3].children[0].children[0].text).toBe('keep');
  });

  it('returns an unchanged tree for valid rectangular tables', () => {
    const value = [
      table(
        [
          row([cell('th', 'A'), cell('th', 'B')]),
          row([cell('td', '1'), cell('td', '2')]),
        ],
        { align: ['left', 'right'] }
      ),
    ] as PlateValueWithTables;

    expect(normalizeTablesInValue(value)).toBe(value);
  });

  it('strips spans, fixes header/body cell types, and pads shorter rows', () => {
    const normalized = normalizeTablesInValue([
      table([
        row([cell('td', 'A', { colSpan: 2 }), cell('th', 'B')]),
        row([cell('th', '1', { rowSpan: 2 }), cell('td', '2'), cell('td', '3')]),
      ]),
    ]);

    expect(rowWidths(normalized)).toEqual([3, 3]);
    expect(normalized[0].children[0].children.map((node) => node.type)).toEqual(['th', 'th', 'th']);
    expect(normalized[0].children[1].children.map((node) => node.type)).toEqual(['td', 'td', 'td']);
    expect(normalized[0].children[0].children[0]).not.toHaveProperty('colSpan');
    expect(normalized[0].children[1].children[0]).not.toHaveProperty('rowSpan');
  });

  it('pads or trims alignment and drops mismatched column sizes', () => {
    const padded = normalizeTablesInValue([
      table(
        [
          row([cell('th', 'A'), cell('th', 'B'), cell('th', 'C')]),
          row([cell('td', '1'), cell('td', '2'), cell('td', '3')]),
        ],
        { align: ['left'], colSizes: [120, 140] }
      ),
    ]) as PlateValueWithTables;
    expect(padded[0].align).toEqual(['left', null, null]);
    expect(padded[0]).not.toHaveProperty('colSizes');

    const trimmed = normalizeTablesInValue([
      table(
        [
          row([cell('th', 'A'), cell('th', 'B')]),
          row([cell('td', '1'), cell('td', '2')]),
        ],
        { align: ['left', 'center', 'right'] }
      ),
    ]) as PlateValueWithTables;
    expect(trimmed[0].align).toEqual(['left', 'center']);
  });
});
