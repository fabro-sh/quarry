import { describe, expect, it } from 'vitest';
import { resolveSuggestions } from './resolve-suggestions';

const value = [
  { type: 'p', children: [
    { text: 'a' },
    { text: 'ins', suggestion: true, suggestion_s1: { id: 's1', type: 'insert', userId: 'user', createdAt: 0 } },
    { text: 'b' },
    { text: 'del', suggestion: true, suggestion_s2: { id: 's2', type: 'remove', userId: 'user', createdAt: 0 } },
  ] },
];

describe('resolveSuggestions', () => {
  it('returns one descriptor per suggestion id with keyId + suggestionId + type', () => {
    const out = resolveSuggestions(value).sort((a, b) => a.suggestionId.localeCompare(b.suggestionId));
    expect(out.map((s) => s.suggestionId)).toEqual(['s1', 's2']);
    expect(out[0]).toMatchObject({ suggestionId: 's1', keyId: 'suggestion_s1', type: 'insert', newText: 'ins' });
    expect(out[1]).toMatchObject({ suggestionId: 's2', keyId: 'suggestion_s2', type: 'remove', text: 'del' });
  });

  it('derives replace when an id has both insert and remove text', () => {
    const v = [{ type: 'p', children: [
      { text: 'old', suggestion: true, suggestion_s3: { id: 's3', type: 'remove', userId: 'user', createdAt: 0 } },
      { text: 'new', suggestion: true, suggestion_s3: { id: 's3', type: 'insert', userId: 'user', createdAt: 0 } },
    ] }];
    const [s] = resolveSuggestions(v);
    expect(s).toMatchObject({ suggestionId: 's3', type: 'replace', text: 'old', newText: 'new' });
  });
});
