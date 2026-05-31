import { describe, expect, it } from 'vitest';
import { currentAuthor } from './identity';

describe('currentAuthor', () => {
  it('defaults to "user"', () => {
    expect(currentAuthor()).toBe('user');
  });
});
