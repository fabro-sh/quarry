import { describe, expect, it } from 'vitest';
import { addComment, addReply, resolveComment, deleteComment, buildThreads, syncSuggestionsFromValue } from './review-store';
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
});
