import { describe, expect, it } from 'vitest';
import { initials } from './format';

describe('initials', () => {
  it('takes the first letter, uppercased', () => {
    expect(initials('user')).toBe('U');
    expect(initials('AI')).toBe('A');
    expect(initials('')).toBe('?');
  });
});
