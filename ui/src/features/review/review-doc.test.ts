import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import {
  REVIEW_ROOT,
  applyReviewMutation,
  bindReviewDoc,
  metaFromReviewMap,
  reconcileReviewMap,
} from './review-doc';
import { addComment, resolveComment, useReviewStore } from './review-store';
import { emptyReviewMeta, type ReviewMeta } from './rfm-types';

const at = '2026-01-01T00:00:00.000Z';

function resetReviewStore() {
  useReviewStore.getState().hydrate(emptyReviewMeta());
  useReviewStore.getState().setActiveId(null);
  useReviewStore.getState().setHoverId(null);
}

beforeEach(resetReviewStore);
afterEach(resetReviewStore);

function seededMeta(): ReviewMeta {
  return addComment(emptyReviewMeta(), 'c1', { by: 'user', at, body: 'tighten this' });
}

describe('review Yjs document binding', () => {
  it('writes mutations through the shared review map and mirrors them into the store', () => {
    const doc = new Y.Doc();
    const observed = vi.fn((meta: ReviewMeta) => useReviewStore.getState().hydrate(meta));
    bindReviewDoc(doc, { isSynced: true, onMeta: observed });

    applyReviewMutation((meta) => addComment(meta, 'c1', { by: 'user', at, body: 'hello' }));

    const mapMeta = metaFromReviewMap(doc.getMap(REVIEW_ROOT));
    expect(mapMeta.comments.c1).toEqual({ by: 'user', at, body: 'hello' });
    expect(useReviewStore.getState().getMeta().comments.c1).toEqual({ by: 'user', at, body: 'hello' });
  });

  it('round-trips editedAt through the shared review map', () => {
    const doc = new Y.Doc();
    const meta: ReviewMeta = {
      comments: {
        c1: {
          by: 'user',
          at,
          body: 'edited body',
          editedAt: '2026-01-01T00:02:00.000Z',
        },
      },
      suggestions: {},
    };

    reconcileReviewMap(doc.getMap(REVIEW_ROOT), meta);

    expect(metaFromReviewMap(doc.getMap(REVIEW_ROOT))).toEqual(meta);
  });

  it('converges review metadata between synced Yjs documents', () => {
    const a = new Y.Doc();
    const b = new Y.Doc();
    reconcileReviewMap(a.getMap(REVIEW_ROOT), seededMeta());
    Y.applyUpdate(b, Y.encodeStateAsUpdate(a));

    bindReviewDoc(a, { isSynced: true, onMeta: () => {} });
    bindReviewDoc(b, { isSynced: true, onMeta: () => {} });

    applyReviewMutation((meta) => resolveComment(meta, 'c1'));
    Y.applyUpdate(a, Y.encodeStateAsUpdate(b));

    expect(metaFromReviewMap(a.getMap(REVIEW_ROOT)).comments.c1.status).toBe('resolved');
  });

  it('treats an empty synced map as authoritative (the server seeds it from rows)', () => {
    const doc = new Y.Doc();
    const observed = vi.fn((next: ReviewMeta) => useReviewStore.getState().hydrate(next));
    useReviewStore.getState().hydrate(seededMeta());

    bindReviewDoc(doc, { isSynced: true, onMeta: observed });

    expect(observed).toHaveBeenCalledWith(emptyReviewMeta());
    expect(useReviewStore.getState().getMeta()).toEqual(emptyReviewMeta());
  });

  it('does not emit unchanged shared metadata echoes', () => {
    const doc = new Y.Doc();
    reconcileReviewMap(doc.getMap(REVIEW_ROOT), seededMeta());
    const observed = vi.fn();
    bindReviewDoc(doc, { isSynced: true, onMeta: observed });

    const comments = doc.getMap(REVIEW_ROOT).get('comments') as Y.Map<unknown>;
    comments.set('c1', { at, by: 'user', body: 'tighten this' });

    expect(observed).toHaveBeenCalledTimes(1);
  });

  it('does not rewrite map entries that only differ by object key order', () => {
    const doc = new Y.Doc();
    const root = doc.getMap(REVIEW_ROOT);
    const comments = new Y.Map<unknown>();
    const suggestions = new Y.Map<unknown>();
    root.set('comments', comments);
    root.set('suggestions', suggestions);
    comments.set('c1', { at, by: 'user', body: 'tighten this' });
    let updateCount = 0;
    doc.on('update', () => {
      updateCount += 1;
    });

    reconcileReviewMap(root, seededMeta());

    expect(updateCount).toBe(0);
  });
});
