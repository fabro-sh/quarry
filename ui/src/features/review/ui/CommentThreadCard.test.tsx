import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ParagraphPlugin, createPlateEditor } from 'platejs/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { reviewKit } from '../../editor/review-kit';
import { addComment, addReply, buildThreads, useReviewStore } from '../review-store';
import { emptyReviewMeta } from '../rfm-types';
import { CommentThreadCard } from './CommentThreadCard';

const at = '2026-01-01T00:00:00.000Z';

// A real editor whose value carries the comment_c1 leaf mark, matching the
// seeded thread id. Deleting the comment must clear this mark from the editor.
function makeEditor() {
  return createPlateEditor({
    plugins: [ParagraphPlugin, ...reviewKit],
    value: [{ type: 'p', children: [{ text: 'see ' }, { text: 'here', comment: true, comment_c1: true }, { text: '.' }] }],
  });
}

function seedThread() {
  let meta = addComment(emptyReviewMeta(), 'c1', { by: 'reviewer', at, body: 'Please tighten this paragraph.' });
  meta = addReply(meta, 'c2', { parentId: 'c1', body: 'Working on it.', by: 'AI', at });
  useReviewStore.getState().hydrate(meta);
  return buildThreads(useReviewStore.getState().getMeta())[0];
}

// A freshly created comment: the root exists (committed with the mark) but has
// no body yet and no replies, exactly as createCommentOnSelection leaves it.
function seedEmptyThread() {
  const meta = addComment(emptyReviewMeta(), 'c1', { by: 'user', at });
  useReviewStore.getState().hydrate(meta);
  return buildThreads(useReviewStore.getState().getMeta())[0];
}

describe('CommentThreadCard', () => {
  beforeEach(() => {
    useReviewStore.getState().hydrate(emptyReviewMeta());
    useReviewStore.getState().setActiveId(null);
    useReviewStore.getState().setHoverId(null);
  });

  afterEach(() => {
    useReviewStore.getState().hydrate(emptyReviewMeta());
    useReviewStore.getState().setActiveId(null);
    useReviewStore.getState().setHoverId(null);
  });

  it('renders the root author and body and the reply body', () => {
    render(<CommentThreadCard thread={seedThread()} editor={makeEditor()} />);

    expect(screen.getByText('reviewer')).toBeInTheDocument();
    expect(screen.getByText('Please tighten this paragraph.')).toBeInTheDocument();
    expect(screen.getByText('Working on it.')).toBeInTheDocument();
  });

  it('adds a reply through the composer', async () => {
    render(<CommentThreadCard thread={seedThread()} editor={makeEditor()} />);

    act(() => {
      useReviewStore.getState().setActiveId('c1');
    });
    await userEvent.type(screen.getByTestId('reply-input'), 'Sounds good');
    await userEvent.click(screen.getByTestId('reply-submit'));

    const replies = Object.values(useReviewStore.getState().getMeta().comments).filter((entry) => entry.re === 'c1');
    expect(replies.map((entry) => ({ by: entry.by, body: entry.body, re: entry.re }))).toContainEqual({
      by: 'user',
      body: 'Sounds good',
      re: 'c1',
    });
  });

  it('fills the empty root body on first submit instead of adding a reply', async () => {
    render(<CommentThreadCard thread={seedEmptyThread()} editor={makeEditor()} />);

    await userEvent.type(screen.getByTestId('reply-input'), 'hello');
    await userEvent.click(screen.getByTestId('reply-submit'));

    const comments = useReviewStore.getState().getMeta().comments;
    expect(comments.c1.body).toBe('hello');
    const replies = Object.values(comments).filter((entry) => entry.re === 'c1');
    expect(replies).toHaveLength(0);
  });

  it('adds a reply once the root already has a body', async () => {
    render(<CommentThreadCard thread={seedThread()} editor={makeEditor()} />);

    act(() => {
      useReviewStore.getState().setActiveId('c1');
    });
    await userEvent.type(screen.getByTestId('reply-input'), 're');
    await userEvent.click(screen.getByTestId('reply-submit'));

    const replies = Object.values(useReviewStore.getState().getMeta().comments).filter((entry) => entry.re === 'c1');
    expect(replies.map((entry) => entry.body)).toContain('re');
  });

  it('discards an empty new comment from the store and clears its leaf mark', async () => {
    const editor = makeEditor();
    render(<CommentThreadCard thread={seedEmptyThread()} editor={editor} />);

    await userEvent.click(screen.getByRole('button', { name: 'Discard comment' }));

    expect(useReviewStore.getState().getMeta().comments).toEqual({});
    expect(JSON.stringify(editor.children)).not.toContain('comment_c1');
  });

  it('submits a reply on Enter', async () => {
    render(<CommentThreadCard thread={seedThread()} editor={makeEditor()} />);

    act(() => {
      useReviewStore.getState().setActiveId('c1');
    });
    await userEvent.type(screen.getByTestId('reply-input'), 'Quick reply{Enter}');

    const bodies = Object.values(useReviewStore.getState().getMeta().comments).map((entry) => entry.body);
    expect(bodies).toContain('Quick reply');
  });

  it('disables submit when the composer is empty', () => {
    render(<CommentThreadCard thread={seedEmptyThread()} editor={makeEditor()} />);

    expect(screen.getByTestId('reply-submit')).toBeDisabled();
  });

  it('hides the reply composer until the card is active', () => {
    render(<CommentThreadCard thread={seedThread()} editor={makeEditor()} />);

    expect(screen.queryByTestId('reply-input')).not.toBeInTheDocument();

    act(() => {
      useReviewStore.getState().setActiveId('c1');
    });

    expect(screen.getByTestId('reply-input')).toBeInTheDocument();
  });

  it('cancels a reply, clearing the draft and deselecting the card', async () => {
    render(<CommentThreadCard thread={seedThread()} editor={makeEditor()} />);
    act(() => {
      useReviewStore.getState().setActiveId('c1');
    });

    await userEvent.type(screen.getByTestId('reply-input'), 'half-written');
    await userEvent.click(screen.getByRole('button', { name: 'Cancel reply' }));

    expect(useReviewStore.getState().activeId).toBeNull();
    const bodies = Object.values(useReviewStore.getState().getMeta().comments).map((entry) => entry.body);
    expect(bodies).not.toContain('half-written');
  });

  it('reveals the reply button only once the input is focused', () => {
    render(<CommentThreadCard thread={seedThread()} editor={makeEditor()} />);
    act(() => {
      useReviewStore.getState().setActiveId('c1');
    });

    expect(screen.queryByTestId('reply-submit')).not.toBeInTheDocument();

    fireEvent.focus(screen.getByTestId('reply-input'));

    expect(screen.getByTestId('reply-submit')).toBeInTheDocument();
  });

  it('marks the comment active on card click', async () => {
    render(<CommentThreadCard thread={seedThread()} editor={makeEditor()} />);

    act(() => {
      fireEvent.click(screen.getByText('Please tighten this paragraph.'));
    });

    expect(useReviewStore.getState().activeId).toBe('c1');
  });

  it('reflects the active id with data-active and the active ring', () => {
    render(<CommentThreadCard thread={seedThread()} editor={makeEditor()} />);
    const card = screen.getByTestId('comment-card');

    expect(card).toHaveAttribute('data-active', 'false');

    act(() => {
      useReviewStore.getState().setActiveId('c1');
    });

    expect(card).toHaveAttribute('data-active', 'true');
    expect(card.className).toContain('ring-accent-ring');
  });

  it('reflects the hover id with data-hover', () => {
    render(<CommentThreadCard thread={seedThread()} editor={makeEditor()} />);
    const card = screen.getByTestId('comment-card');

    expect(card).toHaveAttribute('data-hover', 'false');

    act(() => {
      useReviewStore.getState().setHoverId('c1');
    });

    expect(card).toHaveAttribute('data-hover', 'true');
  });

  it('sets the hover id on card mouse enter and clears it on leave', () => {
    render(<CommentThreadCard thread={seedThread()} editor={makeEditor()} />);
    const card = screen.getByTestId('comment-card');

    fireEvent.mouseEnter(card);
    expect(useReviewStore.getState().hoverId).toBe('c1');

    fireEvent.mouseLeave(card);
    expect(useReviewStore.getState().hoverId).toBeNull();
  });

  it('scrolls into view when it becomes active', () => {
    const scrollIntoView = vi.spyOn(Element.prototype, 'scrollIntoView');
    render(<CommentThreadCard thread={seedThread()} editor={makeEditor()} />);

    expect(scrollIntoView).not.toHaveBeenCalled();

    act(() => {
      useReviewStore.getState().setActiveId('c1');
    });

    expect(scrollIntoView).toHaveBeenCalledWith({ block: 'nearest' });
    scrollIntoView.mockRestore();
  });

  it('resolves the comment from the resolve checkbox', async () => {
    render(<CommentThreadCard thread={seedThread()} editor={makeEditor()} />);

    await userEvent.click(screen.getByTestId('resolve-comment'));

    expect(useReviewStore.getState().getMeta().comments.c1.status).toBe('resolved');
  });

  it('deletes the comment from the store and clears its leaf mark from the editor', async () => {
    const editor = makeEditor();
    render(<CommentThreadCard thread={seedThread()} editor={editor} />);

    await userEvent.click(screen.getByRole('button', { name: 'Comment actions' }));
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Delete' }));

    expect(useReviewStore.getState().getMeta().comments).toEqual({});
    expect(JSON.stringify(editor.children)).not.toContain('comment_c1');
  });

  it('shows a Resolved badge and hides the resolve checkbox when resolved', () => {
    let meta = addComment(emptyReviewMeta(), 'c1', { by: 'reviewer', at, body: 'Done already.' });
    meta = { comments: { c1: { ...meta.comments.c1, status: 'resolved' } }, suggestions: {} };
    useReviewStore.getState().hydrate(meta);
    const thread = buildThreads(useReviewStore.getState().getMeta())[0];

    render(<CommentThreadCard thread={thread} editor={makeEditor()} />);

    expect(screen.getByText('Resolved')).toBeInTheDocument();
    expect(screen.queryByTestId('resolve-comment')).not.toBeInTheDocument();
  });
});
