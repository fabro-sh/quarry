import { describe, expect, it } from 'vitest';
import { reviewKit } from './review-kit';

describe('reviewKit', () => {
  it('registers comment and suggestion plugins', () => {
    const keys = reviewKit.map((p) => p.key);
    expect(keys).toContain('comment');
    expect(keys).toContain('suggestion');
  });
});
