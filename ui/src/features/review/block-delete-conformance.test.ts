import { slateNodesToInsertDelta, yTextToSlateElement } from '@slate-yjs/core';
import { NodeApi, type Descendant, type Value } from 'platejs';
import { ParagraphPlugin, createPlateEditor, type PlateEditor } from 'platejs/react';
import { beforeEach, describe, expect, it } from 'vitest';
import * as Y from 'yjs';

import { KNOWN_BLOCK_TYPES } from '../editor/block-capabilities';
import {
  markdownToPlateValue,
  plateValueToMarkdown,
  type PlateValue,
} from '../editor/markdown-codec';
import { reviewKit } from '../editor/review-kit';
import {
  BLOCK_DELETE_CASES,
  BLOCK_DELETE_FIXTURE_MARKDOWN,
  BLOCK_DELETE_PRESERVED_MARKERS,
  assertBlockDeleteRegistryCoverage,
  interleavedBlockDeleteCases,
  type BlockDeleteConformanceCase,
} from './__fixtures__/block-delete-conformance';
import { acceptSuggestionById, rejectSuggestionById } from './accept-reject';
import { resolveSuggestions } from './resolve-suggestions';
import { useReviewStore } from './review-store';
import { emptyReviewMeta } from './rfm-types';

const CREATED_AT = Date.parse('2026-01-01T00:00:00.000Z');

beforeEach(() => useReviewStore.getState().hydrate(emptyReviewMeta()));

describe('block-delete conformance', () => {
  it('requires a fixture for every registered block type', () => {
    expect(() => assertBlockDeleteRegistryCoverage(KNOWN_BLOCK_TYPES)).not.toThrow();
  });

  it.each(BLOCK_DELETE_CASES)(
    'round-trips, accepts, rejects, and serializes $name blocks',
    (fixture) => {
      const accepted = buildSuggestedEditor(fixture);

      acceptSuggestionById(accepted.editor, suggestionId(fixture));

      expect(resolveSuggestions(accepted.editor.children)).toHaveLength(0);
      expectSubtreeIds(accepted.editor, accepted.subtreeIds, false);
      const acceptedMarkdown = plateValueToMarkdown(accepted.editor.children as PlateValue);
      if (fixture.marker) expect(acceptedMarkdown).not.toContain(fixture.marker);
      expectPreservedMarkers(acceptedMarkdown);
      expectNoEmptyListItems(accepted.editor.children);
      expectValidMarkdown(acceptedMarkdown);

      const rejected = buildSuggestedEditor(fixture);
      const beforeReject = plateValueToMarkdown(rejected.editor.children as PlateValue);

      rejectSuggestionById(rejected.editor, suggestionId(fixture));

      expect(resolveSuggestions(rejected.editor.children)).toHaveLength(0);
      expectSubtreeIds(rejected.editor, rejected.subtreeIds, true);
      const rejectedMarkdown = plateValueToMarkdown(rejected.editor.children as PlateValue);
      expect(rejectedMarkdown).toBe(beforeReject);
      if (fixture.marker) expect(rejectedMarkdown).toContain(fixture.marker);
      expectPreservedMarkers(rejectedMarkdown);
      expectNoEmptyListItems(rejected.editor.children);
      expectValidMarkdown(rejectedMarkdown);
    }
  );

  it.each([
    ['document order', [...BLOCK_DELETE_CASES]],
    ['reverse order', [...BLOCK_DELETE_CASES].reverse()],
    ['interleaved order', interleavedBlockDeleteCases()],
  ])('accepts a mixed document in %s without structural leftovers', (_name, order) => {
    const value = fixturePlateValue();
    const subtreeIds = new Map<string, string[]>();
    for (const fixture of BLOCK_DELETE_CASES) {
      subtreeIds.set(fixture.name, addBlockDeleteSuggestion(value, fixture));
    }
    const editor = editorFromYjs(value);
    expect(resolveSuggestions(editor.children)).toHaveLength(BLOCK_DELETE_CASES.length);

    for (const fixture of order) {
      acceptSuggestionById(editor, suggestionId(fixture));
    }

    expect(resolveSuggestions(editor.children)).toHaveLength(0);
    for (const fixture of BLOCK_DELETE_CASES) {
      expectSubtreeIds(editor, subtreeIds.get(fixture.name) ?? [], false);
    }
    const markdown = plateValueToMarkdown(editor.children as PlateValue);
    for (const fixture of BLOCK_DELETE_CASES) {
      if (fixture.marker) expect(markdown).not.toContain(fixture.marker);
    }
    expectPreservedMarkers(markdown);
    expectNoEmptyListItems(editor.children);
    expectValidMarkdown(markdown);
  });
});

function buildSuggestedEditor(fixture: BlockDeleteConformanceCase) {
  const value = fixturePlateValue();
  const subtreeIds = addBlockDeleteSuggestion(value, fixture);
  const editor = editorFromYjs(value);

  expect(resolveSuggestions(editor.children).map((suggestion) => suggestion.suggestionId)).toEqual([
    suggestionId(fixture),
  ]);
  expectSubtreeIds(editor, subtreeIds, true);
  useReviewStore.getState().hydrate({
    comments: {},
    suggestions: {
      [suggestionId(fixture)]: {
        at: new Date(CREATED_AT).toISOString(),
        by: 'Conformance',
        kind: 'block_delete',
      },
    },
  });

  return { editor, subtreeIds };
}

function editorFromYjs(value: PlateValue): PlateEditor {
  const doc = new Y.Doc();
  const root = doc.get('content', Y.XmlText);
  root.applyDelta(slateNodesToInsertDelta(value as Value));
  const roundTripped = yTextToSlateElement(root).children as Value;

  return createPlateEditor({
    plugins: [ParagraphPlugin, ...reviewKit],
    value: roundTripped,
  });
}

function addBlockDeleteSuggestion(
  value: PlateValue,
  fixture: BlockDeleteConformanceCase
): string[] {
  const target = targetNode(value, fixture);
  const subtreeIds: string[] = [];
  let sequence = 0;
  visitElements(target, (element) => {
    const id = `block-delete:${fixture.name}:${sequence}`;
    sequence += 1;
    element.id = id;
    subtreeIds.push(id);
  });
  target.suggestion = {
    createdAt: CREATED_AT,
    id: suggestionId(fixture),
    type: 'remove',
    userId: 'Conformance',
  };
  return subtreeIds;
}

function fixturePlateValue(): PlateValue {
  const value = markdownToPlateValue(BLOCK_DELETE_FIXTURE_MARKDOWN);
  const rawFixture = BLOCK_DELETE_CASES.find((fixture) => fixture.blockType === 'raw_markdown');
  if (!rawFixture?.marker) throw new Error('raw_markdown conformance fixture is missing');

  const placeholders: Record<string, unknown>[] = [];
  for (const node of value) {
    visitElements(node, (element) => {
      if (element.type === 'p' && JSON.stringify(element).includes(rawFixture.marker ?? '')) {
        placeholders.push(element);
      }
    });
  }
  if (placeholders.length !== 1) {
    throw new Error(`expected one raw_markdown placeholder, found ${placeholders.length}`);
  }
  for (const key of Object.keys(placeholders[0])) delete placeholders[0][key];
  Object.assign(placeholders[0], {
    children: [{ text: '' }],
    markdown: rawFixture.source,
    type: 'raw_markdown',
  });
  return value;
}

function targetNode(
  value: PlateValue,
  fixture: BlockDeleteConformanceCase
): Record<string, unknown> {
  const matches: Record<string, unknown>[] = [];
  for (const node of value) {
    visitElements(node, (element) => {
      if (element.type !== fixture.blockType) return;
      if (fixture.marker && !JSON.stringify(element).includes(fixture.marker)) return;
      matches.push(element);
    });
  }
  expect(matches, `one ${fixture.name} target`).toHaveLength(1);
  return matches[0];
}

function visitElements(
  node: Record<string, unknown>,
  visit: (element: Record<string, unknown>) => void
): void {
  if (typeof node.type === 'string') visit(node);
  if (!Array.isArray(node.children)) return;
  for (const child of node.children) {
    if (typeof child === 'object' && child !== null) {
      visitElements(child as Record<string, unknown>, visit);
    }
  }
}

function suggestionId(fixture: BlockDeleteConformanceCase): string {
  return `suggestion:block-delete:${fixture.name}`;
}

function expectSubtreeIds(editor: PlateEditor, ids: readonly string[], present: boolean): void {
  const actual = new Set<string>();
  for (const [node] of editor.api.nodes({ at: [] })) {
    if ('id' in node && typeof node.id === 'string') actual.add(node.id);
  }
  for (const id of ids) {
    expect(actual.has(id), `${id} presence`).toBe(present);
  }
}

function expectPreservedMarkers(markdown: string): void {
  for (const marker of BLOCK_DELETE_PRESERVED_MARKERS) {
    expect(markdown, `${marker} should survive`).toContain(marker);
  }
}

function expectNoEmptyListItems(value: Descendant[]): void {
  const emptyListItems: Record<string, unknown>[] = [];
  for (const node of value) {
    visitElements(node as Record<string, unknown>, (element) => {
      if ('listStyleType' in element && NodeApi.string(element as never).length === 0) {
        emptyListItems.push(element);
      }
    });
  }
  expect(emptyListItems).toEqual([]);
}

function expectValidMarkdown(markdown: string): void {
  expect(() => markdownToPlateValue(markdown)).not.toThrow();
}
