import { describe, expect, test } from 'vitest';

import type { LoadedDocument } from '../api/client';
import { reduceDocumentState, type OpenDocumentIdentity } from './open-document-state';

const IDENTITY: OpenDocumentIdentity = {
  documentId: 'doc-1',
  library: 'notes',
  path: 'daily.md',
  scope: 'library',
};

function loaded(overrides: Partial<LoadedDocument> = {}): LoadedDocument {
  return {
    content: '# Initial',
    contentType: 'text/markdown',
    documentId: 'doc-1',
    etag: '"v1"',
    path: 'daily.md',
    ...overrides,
  };
}

describe('open document reducer', () => {
  test('preserves the live Markdown mirror across persisted-head refreshes', () => {
    const opened = reduceDocumentState(
      { type: 'closed' },
      { type: 'document-loaded', document: loaded(), identity: IDENTITY }
    );
    const edited = reduceDocumentState(opened, {
      type: 'mirror-changed',
      content: '# Local edit',
    });
    const refreshed = reduceDocumentState(edited, {
      type: 'document-loaded',
      document: loaded({ content: '# Canonical checkpoint', etag: '"v2"' }),
      identity: IDENTITY,
    });

    expect(refreshed).toMatchObject({ content: '# Local edit', etag: '"v2"' });
  });

  test('resets state when the same path is recreated with a new document identity', () => {
    const opened = reduceDocumentState(
      { type: 'closed' },
      { type: 'document-loaded', document: loaded(), identity: IDENTITY }
    );
    const recreatedIdentity = { ...IDENTITY, documentId: 'doc-2' };
    const recreated = reduceDocumentState(opened, {
      type: 'document-loaded',
      document: loaded({ content: '# Recreated', documentId: 'doc-2', etag: '"v1-new"' }),
      identity: recreatedIdentity,
    });

    expect(recreated).toMatchObject({
      content: '# Recreated',
      currentDiffOpen: false,
      identity: recreatedIdentity,
      selectedVersionId: null,
    });
  });

  test('refreshes raw text content because it has no live session mirror', () => {
    const rawIdentity = { ...IDENTITY, documentId: 'raw-1', path: 'data.json' };
    const opened = reduceDocumentState(
      { type: 'closed' },
      {
        type: 'document-loaded',
        document: loaded({
          content: '{"value":1}',
          contentType: 'application/json',
          documentId: 'raw-1',
          path: 'data.json',
        }),
        identity: rawIdentity,
      }
    );
    const refreshed = reduceDocumentState(opened, {
      type: 'document-loaded',
      document: loaded({
        content: '{"value":2}',
        contentType: 'application/json',
        documentId: 'raw-1',
        etag: '"v2"',
        path: 'data.json',
      }),
      identity: rawIdentity,
    });

    expect(refreshed).toMatchObject({ content: '{"value":2}', etag: '"v2"' });
  });
});
