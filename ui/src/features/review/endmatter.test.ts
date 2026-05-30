import { describe, expect, it } from 'vitest';
import { splitEndmatter } from './endmatter';

describe('splitEndmatter', () => {
  it('returns null meta when there is no trailing endmatter', () => {
    const md = '# Title\n\nA paragraph.\n';
    expect(splitEndmatter(md)).toEqual({ body: md, meta: null });
  });

  it('splits a trailing review endmatter block with a comments map', () => {
    const md = 'Hello {==world==}{>>note<<}{#c1}.\n\n---\ncomments:\n  c1:\n    by: user\n    at: "2026-04-28T12:00:00.000Z"\n';
    const result = splitEndmatter(md);
    expect(result.body).toBe('Hello {==world==}{>>note<<}{#c1}.');
    expect(result.meta).toEqual({
      comments: { c1: { by: 'user', at: '2026-04-28T12:00:00.000Z' } },
      suggestions: {},
    });
  });

  it('does NOT treat an ordinary trailing --- + YAML as review endmatter', () => {
    const md = '# Notes\n\nSome prose.\n\n---\ntitle: My Doc\nauthor: Jane\n';
    expect(splitEndmatter(md)).toEqual({ body: md, meta: null });
  });

  it('uses only the final --- block as endmatter', () => {
    const md = 'Intro.\n\n---\n\nMore prose with a divider above.\n\n---\nsuggestions:\n  s1:\n    by: AI\n    at: "2026-04-28T12:00:00.000Z"\n';
    const result = splitEndmatter(md);
    expect(result.body).toBe('Intro.\n\n---\n\nMore prose with a divider above.');
    expect(result.meta?.suggestions.s1).toEqual({ by: 'AI', at: '2026-04-28T12:00:00.000Z' });
  });
});
