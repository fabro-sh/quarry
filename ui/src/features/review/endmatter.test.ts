import { describe, expect, it } from 'vitest';
import { serializeReviewMeta, splitEndmatter } from './endmatter';
import { emptyReviewMeta } from './rfm-types';

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

describe('serializeReviewMeta', () => {
  it('returns an empty string when there is no review data', () => {
    expect(serializeReviewMeta(emptyReviewMeta())).toBe('');
  });

  it('emits comments and suggestions with deterministic, sorted keys', () => {
    const yaml = serializeReviewMeta({
      comments: {
        c2: { body: 'reply', by: 'AI', at: '2026-04-28T12:05:00.000Z', re: 'c1' },
        c1: { by: 'user', at: '2026-04-28T12:00:00.000Z' },
      },
      suggestions: { s1: { by: 'AI', at: '2026-04-28T12:10:00.000Z' } },
    });
    expect(yaml).toBe(
      [
        'comments:',
        '  c1:',
        '    at: 2026-04-28T12:00:00.000Z',
        '    by: user',
        '  c2:',
        '    at: 2026-04-28T12:05:00.000Z',
        '    body: reply',
        '    by: AI',
        '    re: c1',
        'suggestions:',
        '  s1:',
        '    at: 2026-04-28T12:10:00.000Z',
        '    by: AI',
        '',
      ].join('\n')
    );
  });

  it('is idempotent: serialize is stable across repeated calls', () => {
    const meta = { comments: { c1: { by: 'user', at: '2026-04-28T12:00:00.000Z' } }, suggestions: {} };
    expect(serializeReviewMeta(meta)).toBe(serializeReviewMeta(meta));
  });
});
