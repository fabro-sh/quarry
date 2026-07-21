import { afterEach, describe, expect, it } from 'vitest';
import {
  addComment,
  addReply,
  buildThreads,
  deleteComment,
  editComment,
  mergeReviewMetaPatch,
  removeSuggestion,
  resolveComment,
  syncSuggestionsFromValue,
  useReviewStore,
} from './review-store';
import { reviewToMarkdown } from './rfm-codec';
import { emptyReviewMeta } from './rfm-types';

const at = '2026-01-01T00:00:00.000Z';

describe('review-store reducers', () => {
  it('addComment inserts a root comment entry', () => {
    const meta = addComment(emptyReviewMeta(), 'c1', { by: 'user', at });
    expect(meta.comments.c1).toEqual({ by: 'user', at });
  });

  it('addReply inserts a reply with re + body', () => {
    let meta = addComment(emptyReviewMeta(), 'c1', { by: 'user', at });
    meta = addReply(meta, 'c2', { parentId: 'c1', body: 'sure', by: 'AI', at });
    expect(meta.comments.c2).toEqual({ by: 'AI', at, body: 'sure', re: 'c1' });
  });

  it('resolveComment sets status', () => {
    let meta = addComment(emptyReviewMeta(), 'c1', { by: 'user', at });
    meta = resolveComment(meta, 'c1', 'done');
    expect(meta.comments.c1.status).toBe('resolved');
    expect(meta.comments.c1.resolved).toBe('done');
  });

  it('editComment updates the body and editedAt without mutating the input meta', () => {
    const original = addComment(emptyReviewMeta(), 'c1', { by: 'user', at, body: 'before' });
    const editedAt = '2026-01-01T00:01:00.000Z';
    const next = editComment(original, 'c1', 'after', editedAt);

    expect(next.comments.c1).toEqual({ by: 'user', at, body: 'after', editedAt });
    expect(original.comments.c1).toEqual({ by: 'user', at, body: 'before' });
  });

  it('deleteComment removes a comment and its replies', () => {
    let meta = addComment(emptyReviewMeta(), 'c1', { by: 'user', at });
    meta = addReply(meta, 'c2', { parentId: 'c1', body: 'x', by: 'AI', at });
    meta = deleteComment(meta, 'c1');
    expect(meta.comments).toEqual({});
  });

  it('does not mutate the input meta', () => {
    const original = emptyReviewMeta();
    addComment(original, 'c1', { by: 'user', at });
    expect(original.comments).toEqual({});
  });

  it('buildThreads groups replies under their root, sorted', () => {
    let meta = addComment(emptyReviewMeta(), 'c1', { by: 'user', at });
    meta = addReply(meta, 'c2', { parentId: 'c1', body: 'r1', by: 'AI', at });
    const threads = buildThreads(meta);
    expect(threads).toHaveLength(1);
    expect(threads[0].id).toBe('c1');
    expect(threads[0].replies.map((r) => r.id)).toEqual(['c2']);
  });

  it('syncSuggestionsFromValue adds entries for marks missing from meta', () => {
    const value = [{ type: 'p', children: [{ text: 'x', suggestion: true, suggestion_s1: { id: 's1', type: 'insert', userId: 'user', createdAt: Date.parse(at) } }] }];
    const meta = syncSuggestionsFromValue(emptyReviewMeta(), value);
    expect(meta.suggestions.s1).toEqual({ by: 'user', at });
  });

  it('syncSuggestionsFromValue does not override an existing entry', () => {
    const value = [{ type: 'p', children: [{ text: 'x', suggestion: true, suggestion_s1: { id: 's1', type: 'insert', userId: 'someone-else', createdAt: 0 } }] }];
    const meta = syncSuggestionsFromValue({ comments: {}, suggestions: { s1: { by: 'AI', at } } }, value);
    expect(meta.suggestions.s1).toEqual({ by: 'AI', at });
  });

  it('syncSuggestionsFromValue records block-delete semantics from an element', () => {
    const value = [{
      type: 'p',
      suggestion: { id: 's1', type: 'remove', userId: 'user', createdAt: Date.parse(at) },
      children: [{ text: 'remove me' }],
    }];

    const meta = syncSuggestionsFromValue(emptyReviewMeta(), value);

    expect(meta.suggestions.s1).toEqual({ by: 'user', at, kind: 'block_delete' });
  });

  it('mergeReviewMetaPatch preserves an injected root comment body for markdown serialization', () => {
    const value = [
      {
        type: 'p',
        children: [{ text: 'target', comment: true, comment_c1: true }],
      },
    ];
    const meta = mergeReviewMetaPatch(emptyReviewMeta(), {
      comments: {
        c1: { by: 'ai:codex', at, body: 'Needs support.' },
      },
    });

    expect(reviewToMarkdown(value, meta)).toContain(
      '{==target==}{>>Needs support.<<}{#c1}'
    );
  });

  it('mergeReviewMetaPatch applies upserts and removals', () => {
    const meta = mergeReviewMetaPatch(
      {
        comments: {
          c1: { by: 'user', at },
          r1: { by: 'user', at, body: 'reply', re: 'c1' },
          c2: { by: 'user', at },
        },
        suggestions: {
          s1: { by: 'AI', at },
          s2: { by: 'AI', at },
        },
      },
      {
        comments: {
          c3: { by: 'ai:codex', at, body: 'new comment' },
        },
        suggestions: {
          s3: { by: 'ai:codex', at },
        },
        removeComments: ['c1', 'r1'],
        removeSuggestions: ['s1'],
      }
    );

    expect(meta.comments).toEqual({
      c2: { by: 'user', at },
      c3: { by: 'ai:codex', at, body: 'new comment' },
    });
    expect(meta.suggestions).toEqual({
      s2: { by: 'AI', at },
      s3: { by: 'ai:codex', at },
    });
  });

  it('removeSuggestion deletes one suggestion entry and its replies without touching the rest', () => {
    const meta = removeSuggestion(
      {
        comments: {
          r1: { by: 'user', at, body: 'question', re: 's1' },
          r2: { by: 'user', at, body: 'keep', re: 's2' },
        },
        suggestions: {
          s1: { by: 'AI', at },
          s2: { by: 'user', at },
        },
      },
      's1'
    );

    expect(meta.suggestions).toEqual({ s2: { by: 'user', at } });
    expect(meta.comments).toEqual({ r2: { by: 'user', at, body: 'keep', re: 's2' } });
  });
});

describe('review-store active/hover', () => {
  afterEach(() => {
    useReviewStore.getState().setActiveId(null);
    useReviewStore.getState().setHoverId(null);
  });

  it('sets and clears activeId / hoverId', () => {
    useReviewStore.getState().setActiveId('c1');
    expect(useReviewStore.getState().activeId).toBe('c1');
    useReviewStore.getState().setHoverId('s1');
    expect(useReviewStore.getState().hoverId).toBe('s1');
    useReviewStore.getState().setActiveId(null);
    expect(useReviewStore.getState().activeId).toBeNull();
  });
});
