import { render, screen } from '@testing-library/react';
import { isCommentKey } from '@platejs/comment';
import { CommentPlugin } from '@platejs/comment/react';
import { SuggestionPlugin } from '@platejs/suggestion/react';
import { HighlightPlugin } from '@platejs/basic-nodes/react';
import { NodeApi } from 'platejs';
import { ParagraphPlugin, createPlateEditor } from 'platejs/react';
import { describe, expect, it, vi } from 'vitest';
import { createCommentOnSelection, PlateMarkdownEditor } from './PlateMarkdownEditor';
import { useReviewStore } from '../review/review-store';

const DOC = 'See {==here==}{>>fix this<<}{#c1}.\n\n---\ncomments:\n  c1:\n    at: "2026-01-01T00:00:00.000Z"\n    by: user\n';
const DOC_B = 'A different document with no review marks.\n';

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

  it('does not fire onChange when the document is swapped out (pure load)', () => {
    const onChange = vi.fn();
    const { rerender } = render(<PlateMarkdownEditor content={DOC} onChange={onChange} />);
    // Loading a new document is not a user edit: hydrating the store
    // synchronously notifies the save subscription, which must short-circuit on
    // the equality guard rather than reporting a spurious change. (Before the
    // fix this fired onChange once with DOC_B's serialized text.)
    onChange.mockClear();
    rerender(<PlateMarkdownEditor content={DOC_B} onChange={onChange} />);
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe('createCommentOnSelection', () => {
  it('marks the selected text with a comment id and records a store entry', () => {
    useReviewStore.getState().hydrate({ comments: {}, suggestions: {} });
    const editor = createPlateEditor({
      plugins: [ParagraphPlugin, CommentPlugin, SuggestionPlugin, HighlightPlugin],
      value: [{ type: 'p', children: [{ text: 'Comment this word.' }] }],
    });
    // Select the word "word" (offsets 13–17 of "Comment this word.").
    editor.tf.select({
      anchor: { path: [0, 0], offset: 13 },
      focus: { path: [0, 0], offset: 17 },
    });

    createCommentOnSelection(editor);

    // (a) A comment_<id> mark now covers the selected text.
    const commented = editor.api.nodes({
      at: [],
      match: (node) => Object.keys(node).some((key) => isCommentKey(key)),
    });
    const commentedText = Array.from(commented, ([node]) => NodeApi.string(node));
    expect(commentedText).toEqual(['word']);

    // (b) The store gained one comment entry authored by the local user.
    const comments = useReviewStore.getState().getMeta().comments;
    const ids = Object.keys(comments);
    expect(ids).toHaveLength(1);
    expect(comments[ids[0]].by).toBe('user');
  });
});
