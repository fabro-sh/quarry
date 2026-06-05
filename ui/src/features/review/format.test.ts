import { describe, expect, it } from 'vitest';
import { firstWord, initials } from './format';

describe('initials', () => {
  it('takes the first letter, uppercased', () => {
    expect(initials('user')).toBe('U');
    expect(initials('AI')).toBe('A');
    expect(initials('')).toBe('?');
  });
});

describe('firstWord', () => {
  it('keeps only the first word of a multi-word name', () => {
    expect(firstWord('Claude Sonnet')).toBe('Claude');
    expect(firstWord('Bryan Helmkamp')).toBe('Bryan');
  });

  it('leaves a single-word name unchanged', () => {
    expect(firstWord('user')).toBe('user');
  });
});
