import { describe, expect, it } from 'vitest';
import { applyCriticMarkup } from './apply-critic-markup';
import { emptyReviewMeta } from './rfm-types';

const p = (children: unknown[]) => ({ type: 'p', children });

describe('applyCriticMarkup', () => {
  it('parses an insert suggestion and reads metadata from meta', () => {
    const meta = { comments: {}, suggestions: { s1: { by: 'AI', at: '2026-01-01T00:00:00.000Z' } } };
    const { value } = applyCriticMarkup([p([{ text: 'add {++more++}{#s1}' }])], meta);
    expect(value).toEqual([p([{ text: 'add ' }, { text: 'more', suggestion: true, suggestion_s1: { id: 's1', type: 'insert', userId: 'AI', createdAt: expect.any(Number) } }])]);
  });

  it('parses a remove suggestion', () => {
    const { value } = applyCriticMarkup([p([{ text: 'drop {--this--}{#s2}' }])], { comments: {}, suggestions: { s2: { by: 'user', at: 'x' } } });
    const leaf = (value[0] as { children: Record<string, unknown>[] }).children[1];
    expect(leaf.text).toBe('this');
    expect(leaf.suggestion).toBe(true);
    expect((leaf.suggestion_s2 as { type: string }).type).toBe('remove');
  });

  it('splits a substitution into a remove leaf and an insert leaf sharing the id', () => {
    const { value } = applyCriticMarkup([p([{ text: 'use {~~rough~>specific~~}{#s3}' }])], { comments: {}, suggestions: { s3: { by: 'AI', at: 'x' } } });
    const children = (value[0] as { children: Record<string, unknown>[] }).children;
    const remove = children.find((c) => (c.suggestion_s3 as { type?: string })?.type === 'remove');
    const insert = children.find((c) => (c.suggestion_s3 as { type?: string })?.type === 'insert');
    expect(remove?.text).toBe('rough');
    expect(insert?.text).toBe('specific');
  });

  it('parses a comment anchor and lifts the inline body into meta', () => {
    const meta = { comments: { c1: { by: 'user', at: '2026-01-01T00:00:00.000Z' } }, suggestions: {} };
    const { value, meta: outMeta } = applyCriticMarkup([p([{ text: 'see {==here==}{>>fix this<<}{#c1}' }])], meta);
    const leaf = (value[0] as { children: Record<string, unknown>[] }).children[1];
    expect(leaf.text).toBe('here');
    expect(leaf.comment).toBe(true);
    expect(leaf.comment_c1).toBe(true);
    expect(outMeta.comments.c1.body).toBe('fix this');
  });

  it('parses a bare highlight as a highlight mark (no comment)', () => {
    const { value } = applyCriticMarkup([p([{ text: 'pick {==this==} please' }])], emptyReviewMeta());
    expect(value).toEqual([p([{ text: 'pick ' }, { text: 'this', highlight: true }, { text: ' please' }])]);
  });

  it('synthesizes an id when a marker has none', () => {
    const { value, meta } = applyCriticMarkup([p([{ text: 'add {++x++}' }])], emptyReviewMeta());
    const leaf = (value[0] as { children: Record<string, unknown>[] }).children[1];
    const data = Object.entries(leaf).find(([k]) => k.startsWith('suggestion_'));
    expect(data).toBeTruthy();
    expect(Object.keys(meta.suggestions)).toHaveLength(1);
  });

  it('leaves CriticMarkup inside a code leaf literal', () => {
    const { value } = applyCriticMarkup([p([{ text: '{++x++}', code: true }])], emptyReviewMeta());
    expect(value).toEqual([p([{ text: '{++x++}', code: true }])]);
  });
});
