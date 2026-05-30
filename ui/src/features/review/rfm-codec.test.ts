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
});
