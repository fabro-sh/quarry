import { createSlateEditor } from 'platejs';
import { describe, expect, it, vi } from 'vitest';
import { serializeReviewBody } from './review-md-rules';
import { emptyReviewMeta } from './rfm-types';

vi.mock('platejs', async () => {
  const actual = await vi.importActual<typeof import('platejs')>('platejs');
  return {
    ...actual,
    createSlateEditor: vi.fn(actual.createSlateEditor),
  };
});

describe('serializeReviewBody', () => {
  it('emits an insert suggestion as {++text++}{#id}', () => {
    const value = [
      { type: 'p', children: [{ text: 'add ' }, { text: 'more', suggestion: true, suggestion_s1: { id: 's1', type: 'insert', userId: 'AI', createdAt: 0 } }] },
    ];
    expect(serializeReviewBody(value, emptyReviewMeta())).toBe('add {++more++}{#s1}');
  });

  it('emits a remove suggestion as {--text--}{#id}', () => {
    const value = [
      { type: 'p', children: [{ text: 'drop ', }, { text: 'this', suggestion: true, suggestion_s2: { id: 's2', type: 'remove', userId: 'user', createdAt: 0 } }] },
    ];
    expect(serializeReviewBody(value, emptyReviewMeta())).toBe('drop {--this--}{#s2}');
  });

  it('emits an update suggestion as plain text (no CriticMarkup)', () => {
    const value = [
      { type: 'p', children: [{ text: 'keep ' }, { text: 'this', suggestion: true, suggestion_s3: { id: 's3', type: 'update', userId: 'AI', createdAt: 0 } }] },
    ];
    expect(serializeReviewBody(value, emptyReviewMeta())).toBe('keep this');
  });

  it('emits a comment as {==anchor==}{>>body<<}{#id} using the body from meta', () => {
    const value = [{ type: 'p', children: [{ text: 'see ' }, { text: 'here', comment: true, comment_c1: true }] }];
    const meta = { comments: { c1: { by: 'user', at: 'x', body: 'fix this' } }, suggestions: {} };
    expect(serializeReviewBody(value, meta)).toBe('see {==here==}{>>fix this<<}{#c1}');
  });

  it('reuses the serializer editor for equivalent review metadata', () => {
    const id = `cache-${Date.now()}`;
    const value = [{ type: 'p', children: [{ text: 'see ' }, { text: 'here', comment: true, [`comment_${id}`]: true }] }];
    const meta = () => ({ comments: { [id]: { by: 'user', at: 'x', body: 'same' } }, suggestions: {} });
    vi.mocked(createSlateEditor).mockClear();

    expect(serializeReviewBody(value, meta())).toBe(`see {==here==}{>>same<<}{#${id}}`);
    expect(serializeReviewBody(value, meta())).toBe(`see {==here==}{>>same<<}{#${id}}`);

    expect(createSlateEditor).toHaveBeenCalledTimes(1);
  });
});
