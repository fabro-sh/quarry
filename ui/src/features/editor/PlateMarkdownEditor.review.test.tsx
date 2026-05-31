import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { PlateMarkdownEditor } from './PlateMarkdownEditor';

const DOC = 'See {==here==}{>>fix this<<}{#c1}.\n\n---\ncomments:\n  c1:\n    at: "2026-01-01T00:00:00.000Z"\n    by: user\n';

describe('PlateMarkdownEditor review round-trip', () => {
  it('renders a commented range as a comment mark', () => {
    render(<PlateMarkdownEditor content={DOC} onChange={vi.fn()} />);
    // Plate's CommentPlugin styles a comment leaf with the `slate-comment`
    // class; the commented "here" range should carry it while plain text does
    // not.
    expect(screen.getByText('here')).toBeInTheDocument();
    const marked = document.querySelector('.slate-comment');
    expect(marked).not.toBeNull();
    expect(marked).toHaveTextContent('here');
  });
});
