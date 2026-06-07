import { describe, expect, it } from 'vitest';
import { markdownToReview, reviewToMarkdown } from './rfm-codec';

describe('rfm-codec round-trip', () => {
  it('round-trips a comment with endmatter', () => {
    const md = 'See {==here==}{>>fix this<<}{#c1}.\n\n---\ncomments:\n  c1:\n    at: "2026-01-01T00:00:00.000Z"\n    by: user\n';
    const { value, meta } = markdownToReview(md);
    expect(meta.comments.c1.by).toBe('user');
    const out = reviewToMarkdown(value, meta);
    expect(out).toContain('{==here==}{>>fix this<<}{#c1}');
    expect(out).toContain('comments:');
  });

  it('round-trips a substitution suggestion', () => {
    const md = 'Use {~~rough~>specific~~}{#s1} wording.\n\n---\nsuggestions:\n  s1:\n    at: "2026-01-01T00:00:00.000Z"\n    by: AI\n';
    const { value, meta } = markdownToReview(md);
    const out = reviewToMarkdown(value, meta);
    expect(out).toContain('{~~rough~>specific~~}{#s1}');
  });

  it('is idempotent: parse→serialize→parse→serialize is stable', () => {
    const md = 'Use {~~rough~>specific~~}{#s1} wording.\n\n---\nsuggestions:\n  s1:\n    at: "2026-01-01T00:00:00.000Z"\n    by: AI\n';
    const r1 = markdownToReview(md);
    const first = reviewToMarkdown(r1.value, r1.meta);
    const r2 = markdownToReview(first);
    const second = reviewToMarkdown(r2.value, r2.meta);
    expect(second).toBe(first);
  });

  it('only emits endmatter entries for ids still present as marks (orphan prune)', () => {
    const md = 'Plain text, no markers.\n\n---\ncomments:\n  c1:\n    at: "2026-01-01T00:00:00.000Z"\n    by: user\n';
    const { value, meta } = markdownToReview(md);
    const out = reviewToMarkdown(value, meta);
    expect(out).not.toContain('comments:');
    expect(out.trim()).toBe('Plain text, no markers.');
  });

  it('does not duplicate a root comment body in both inline and endmatter', () => {
    const md = 'See {==here==}{>>fix this<<}{#c1}.\n\n---\ncomments:\n  c1:\n    at: "2026-01-01T00:00:00.000Z"\n    by: user\n';
    const { value, meta } = markdownToReview(md);
    const out = reviewToMarkdown(value, meta);
    expect(out).toContain('{>>fix this<<}');
    expect(out).not.toContain('body:');
  });

  it('normalizes explicit orphan ids without random metadata', () => {
    const md = 'See {==here==}{>>fix this<<}{#c1}. Add {++x++}{#s1}.\n';
    const { meta } = markdownToReview(md);
    expect(meta.comments.c1).toEqual({ by: 'unknown', at: '', body: 'fix this' });
    expect(meta.suggestions.s1).toEqual({ by: 'unknown', at: '' });
  });

  it('does not create metadata for anonymous review markers', () => {
    const md = 'See {==here==}{>>fix this<<}. Add {++x++}.\n';
    const { meta } = markdownToReview(md);
    expect(meta.comments).toEqual({});
    expect(meta.suggestions).toEqual({});
  });

  it('round-trips a GFM table with alignment through the review save path', () => {
    const md = '| A | B |\n| :-- | --: |\n| x | y |\n';
    const { value, meta } = markdownToReview(md);
    expect((value[0] as { type?: string }).type).toBe('table');
    expect((value[0] as { align?: unknown }).align).toEqual(['left', 'right']);
    const out = reviewToMarkdown(value, meta);
    expect(out).toContain('| x');
    // Alignment survives the review save path (assert via re-parse, robust to GFM's
    // minimal delimiter form).
    expect((markdownToReview(out).value[0] as { align?: unknown }).align).toEqual(['left', 'right']);
  });

  it('repairs malformed table values through the review save path', () => {
    const value = [
      {
        type: 'table',
        align: ['left', 'center', 'right'],
        children: [
          {
            type: 'tr',
            children: [
              { type: 'td', children: [{ type: 'p', children: [{ text: 'A' }] }] },
              { type: 'th', children: [{ type: 'p', children: [{ text: 'B' }] }] },
            ],
          },
          {
            type: 'tr',
            children: [
              { type: 'td', children: [{ type: 'p', children: [{ text: '1' }] }] },
              { type: 'td', rowSpan: 2, children: [{ type: 'p', children: [{ text: '2' }] }] },
              { type: 'td', children: [{ type: 'p', children: [{ text: 'keep' }] }] },
            ],
          },
        ],
      },
    ];

    const out = reviewToMarkdown(value as never, { comments: {}, suggestions: {} });
    const reparsed = markdownToReview(out).value;

    expect(out).toContain('keep');
    expect((reparsed[0] as { align?: unknown }).align).toEqual(['left', 'center', 'right']);
    expect(
      ((reparsed[0] as { children: Array<{ children: unknown[] }> }).children).map(
        (row) => row.children.length
      )
    ).toEqual([3, 3]);
  });
});
