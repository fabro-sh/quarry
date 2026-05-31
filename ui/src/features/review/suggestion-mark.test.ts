import { describe, expect, it } from 'vitest';
import { readSuggestionMark } from './suggestion-mark';

describe('readSuggestionMark', () => {
  it('reads the suggestion data object off a leaf', () => {
    expect(readSuggestionMark({ text: 'x', suggestion: true, suggestion_s1: { id: 's1', type: 'insert', userId: 'AI', createdAt: 5 } }))
      .toEqual({ id: 's1', type: 'insert', userId: 'AI', createdAt: 5 });
  });
  it('returns null when there is no suggestion key', () => {
    expect(readSuggestionMark({ text: 'x' })).toBeNull();
  });
  it('ignores a malformed suggestion value', () => {
    expect(readSuggestionMark({ text: 'x', suggestion_s2: { type: 'insert' } })).toBeNull();
  });
});
