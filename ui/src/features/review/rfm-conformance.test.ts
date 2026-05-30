import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { markdownToReview, reviewToMarkdown } from './rfm-codec';
import { emptyReviewMeta } from './rfm-types';

// Hoist import.meta.url: Vitest's transform mishandles it as an inline new URL() second arg, breaking fixture path resolution.
const here = import.meta.url;
const fixture = (name: string) =>
  readFileSync(fileURLToPath(new URL(`./__fixtures__/${name}`, here)), 'utf-8');

describe('RFM conformance', () => {
  it('keeps a threaded reply (re:) in metadata and round-trips its parent anchor', () => {
    const { value, meta } = markdownToReview(fixture('threaded-comment.md'));
    expect(meta.comments.c2.re).toBe('c1');
    const out = reviewToMarkdown(value, meta);
    expect(out).toContain('{==this sentence==}');
    expect(out).toContain('re: c1');
  });

  it('treats CriticMarkup inside inline code and fenced blocks as literal text', () => {
    const { value } = markdownToReview(fixture('code-literal.md'));
    const out = reviewToMarkdown(value, emptyReviewMeta());
    expect(out).toContain('`{==not a comment==}`');
    expect(out).toContain('{++not a suggestion++}');
  });

  it('treats an unclosed marker as literal text (no crash)', () => {
    const { value, meta } = markdownToReview('A stray {++ open marker.\n');
    expect(meta.suggestions).toEqual({});
    expect(reviewToMarkdown(value, meta)).toContain('{++ open marker.');
  });
});
