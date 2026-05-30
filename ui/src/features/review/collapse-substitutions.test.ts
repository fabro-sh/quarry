import { describe, expect, it } from 'vitest';
import { collapseSubstitutions, expandSubstitutions } from './collapse-substitutions';

describe('collapseSubstitutions', () => {
  it('collapses an adjacent remove+insert sharing an id', () => {
    expect(collapseSubstitutions('use {--rough--}{#s1}{++specific++}{#s1} wording')).toBe(
      'use {~~rough~>specific~~}{#s1} wording'
    );
  });

  it('also collapses insert-before-remove order', () => {
    expect(collapseSubstitutions('use {++specific++}{#s1}{--rough--}{#s1} wording')).toBe(
      'use {~~rough~>specific~~}{#s1} wording'
    );
  });

  it('leaves standalone insert/delete untouched', () => {
    expect(collapseSubstitutions('add {++x++}{#s1} and drop {--y--}{#s2}')).toBe(
      'add {++x++}{#s1} and drop {--y--}{#s2}'
    );
  });

  it('does not merge a remove+insert with different ids', () => {
    const input = '{--old--}{#s1}{++new++}{#s2}';
    expect(collapseSubstitutions(input)).toBe(input);
  });

  it('does not merge an insert+remove with different ids', () => {
    const input = '{++new++}{#s1}{--old--}{#s2}';
    expect(collapseSubstitutions(input)).toBe(input);
  });
});

describe('expandSubstitutions', () => {
  it('expands a substitution into the id-paired remove+insert form', () => {
    expect(expandSubstitutions('use {~~rough~>specific~~}{#s1} wording')).toBe(
      'use {--rough--}{#s1}{++specific++}{#s1} wording'
    );
  });

  it('round-trips back to the substitution via collapse', () => {
    const input = 'use {~~rough~>specific~~}{#s1} wording';
    expect(collapseSubstitutions(expandSubstitutions(input))).toBe(input);
  });

  it('synthesizes a shared id when the token has none', () => {
    const out = expandSubstitutions('{~~a~>b~~}');
    const match = /\{--a--\}\{#([A-Za-z0-9_-]+)\}\{\+\+b\+\+\}\{#([A-Za-z0-9_-]+)\}/.exec(out);
    expect(match).not.toBeNull();
    expect(match?.[1]).toBe(match?.[2]);
  });

  it('leaves text without substitutions untouched', () => {
    expect(expandSubstitutions('plain {==hi==} text')).toBe('plain {==hi==} text');
  });
});
