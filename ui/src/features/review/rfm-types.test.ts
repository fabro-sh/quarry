import { describe, expect, it } from 'vitest';
import { emptyReviewMeta, isEmptyReviewMeta } from './rfm-types';

describe('rfm-types', () => {
  it('emptyReviewMeta has empty comment and suggestion maps', () => {
    expect(emptyReviewMeta()).toEqual({ comments: {}, suggestions: {} });
  });

  it('isEmptyReviewMeta is true only when both maps are empty', () => {
    expect(isEmptyReviewMeta(emptyReviewMeta())).toBe(true);
    expect(isEmptyReviewMeta({ comments: { c1: { by: 'user', at: 'x' } }, suggestions: {} })).toBe(false);
    expect(isEmptyReviewMeta({ comments: {}, suggestions: { s1: { by: 'AI', at: 'x' } } })).toBe(false);
  });
});
