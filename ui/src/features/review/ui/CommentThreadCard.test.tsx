import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { addComment, addReply, buildThreads, useReviewStore } from '../review-store';
import { emptyReviewMeta } from '../rfm-types';
import { CommentThreadCard } from './CommentThreadCard';

const at = '2026-01-01T00:00:00.000Z';

function seedThread() {
  let meta = addComment(emptyReviewMeta(), 'c1', { by: 'reviewer', at, body: 'Please tighten this paragraph.' });
  meta = addReply(meta, 'c2', { parentId: 'c1', body: 'Working on it.', by: 'AI', at });
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
    render(<CommentThreadCard thread={seedThread()} />);

    expect(screen.getByText('reviewer')).toBeInTheDocument();
    expect(screen.getByText('Please tighten this paragraph.')).toBeInTheDocument();
    expect(screen.getByText('Working on it.')).toBeInTheDocument();
  });

  it('adds a reply through the composer', async () => {
    render(<CommentThreadCard thread={seedThread()} />);

    await userEvent.type(screen.getByTestId('reply-input'), 'Sounds good');
    await userEvent.click(screen.getByTestId('reply-submit'));

    const replies = Object.values(useReviewStore.getState().getMeta().comments).filter((entry) => entry.re === 'c1');
    expect(replies.map((entry) => ({ by: entry.by, body: entry.body, re: entry.re }))).toContainEqual({
      by: 'user',
      body: 'Sounds good',
      re: 'c1',
    });
  });

  it('submits a reply on Enter without shift', async () => {
    render(<CommentThreadCard thread={seedThread()} />);

    await userEvent.type(screen.getByTestId('reply-input'), 'Quick reply{Enter}');

    const bodies = Object.values(useReviewStore.getState().getMeta().comments).map((entry) => entry.body);
    expect(bodies).toContain('Quick reply');
  });

  it('disables submit when the composer is empty', () => {
    render(<CommentThreadCard thread={seedThread()} />);

    expect(screen.getByTestId('reply-submit')).toBeDisabled();
  });

  it('marks the comment active on card click', async () => {
    render(<CommentThreadCard thread={seedThread()} />);

    act(() => {
      fireEvent.click(screen.getByText('Please tighten this paragraph.'));
    });

    expect(useReviewStore.getState().activeId).toBe('c1');
  });

  it('reflects the active id with data-active and the active ring', () => {
    render(<CommentThreadCard thread={seedThread()} />);
    const card = screen.getByTestId('comment-card');

    expect(card).toHaveAttribute('data-active', 'false');

    act(() => {
      useReviewStore.getState().setActiveId('c1');
    });

    expect(card).toHaveAttribute('data-active', 'true');
    expect(card.className).toContain('ring-accent-ring');
  });

  it('reflects the hover id with data-hover', () => {
    render(<CommentThreadCard thread={seedThread()} />);
    const card = screen.getByTestId('comment-card');

    expect(card).toHaveAttribute('data-hover', 'false');

    act(() => {
      useReviewStore.getState().setHoverId('c1');
    });

    expect(card).toHaveAttribute('data-hover', 'true');
  });

  it('sets the hover id on card mouse enter and clears it on leave', () => {
    render(<CommentThreadCard thread={seedThread()} />);
    const card = screen.getByTestId('comment-card');

    fireEvent.mouseEnter(card);
    expect(useReviewStore.getState().hoverId).toBe('c1');

    fireEvent.mouseLeave(card);
    expect(useReviewStore.getState().hoverId).toBeNull();
  });

  it('scrolls into view when it becomes active', () => {
    const scrollIntoView = vi.spyOn(Element.prototype, 'scrollIntoView');
    render(<CommentThreadCard thread={seedThread()} />);

    expect(scrollIntoView).not.toHaveBeenCalled();

    act(() => {
      useReviewStore.getState().setActiveId('c1');
    });

    expect(scrollIntoView).toHaveBeenCalledWith({ block: 'nearest' });
    scrollIntoView.mockRestore();
  });

  it('resolves the comment from the actions menu', async () => {
    render(<CommentThreadCard thread={seedThread()} />);

    await userEvent.click(screen.getByRole('button', { name: 'Comment actions' }));
    await userEvent.click(await screen.findByTestId('resolve-comment'));

    expect(useReviewStore.getState().getMeta().comments.c1.status).toBe('resolved');
  });

  it('deletes the comment and its replies from the actions menu', async () => {
    render(<CommentThreadCard thread={seedThread()} />);

    await userEvent.click(screen.getByRole('button', { name: 'Comment actions' }));
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Delete' }));

    expect(useReviewStore.getState().getMeta().comments).toEqual({});
  });

  it('shows a Resolved badge and hides the resolve action when resolved', async () => {
    let meta = addComment(emptyReviewMeta(), 'c1', { by: 'reviewer', at, body: 'Done already.' });
    meta = { comments: { c1: { ...meta.comments.c1, status: 'resolved' } }, suggestions: {} };
    useReviewStore.getState().hydrate(meta);
    const thread = buildThreads(useReviewStore.getState().getMeta())[0];

    render(<CommentThreadCard thread={thread} />);

    expect(screen.getByText('Resolved')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Comment actions' }));
    expect(screen.queryByTestId('resolve-comment')).not.toBeInTheDocument();
  });
});
