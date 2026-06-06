import { fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { addComment, resolveComment, useReviewStore } from '../review-store';
import { emptyReviewMeta } from '../rfm-types';
import { CommentsPanel } from './CommentsPanel';

const at = '2026-01-01T00:00:00.000Z';

// One open thread and one resolved thread, the fixture every test starts from.
function seedOpenAndResolved() {
  let meta = emptyReviewMeta();
  meta = addComment(meta, 'c1', { by: 'user', at, body: 'open note' });
  meta = addComment(meta, 'c2', { by: 'user', at, body: 'resolved note' });
  meta = resolveComment(meta, 'c2');
  useReviewStore.getState().hydrate(meta);
}

describe('CommentsPanel', () => {
  beforeEach(() => {
    useReviewStore.getState().hydrate(emptyReviewMeta());
  });

  afterEach(() => {
    useReviewStore.getState().hydrate(emptyReviewMeta());
  });

  it('shows both open and resolved comments under the default All filter', () => {
    seedOpenAndResolved();
    render(<CommentsPanel />);

    expect(screen.getByText('open note')).toBeInTheDocument();
    expect(screen.getByText('resolved note')).toBeInTheDocument();
  });

  it('filters to only open comments', () => {
    seedOpenAndResolved();
    render(<CommentsPanel />);

    fireEvent.click(screen.getByTestId('comments-filter-open'));

    expect(screen.getByText('open note')).toBeInTheDocument();
    expect(screen.queryByText('resolved note')).not.toBeInTheDocument();
  });

  it('filters to only resolved comments', () => {
    seedOpenAndResolved();
    render(<CommentsPanel />);

    fireEvent.click(screen.getByTestId('comments-filter-resolved'));

    // Only the resolved thread remains, so its item is unique and carries the
    // Resolved badge plus the reopen affordance.
    const item = screen.getByTestId('comments-panel-item');
    expect(within(item).getByText('resolved note')).toBeInTheDocument();
    expect(within(item).getByText('Resolved')).toBeInTheDocument();
    expect(within(item).getByTestId('reopen-comment')).toBeInTheDocument();
    expect(screen.queryByText('open note')).not.toBeInTheDocument();
  });

  it('reopens a resolved comment, clearing its resolved status', () => {
    seedOpenAndResolved();
    render(<CommentsPanel />);

    fireEvent.click(screen.getByTestId('comments-filter-resolved'));
    fireEvent.click(screen.getByTestId('reopen-comment'));

    // Reopened: it leaves the resolved filter and the store no longer marks it.
    expect(screen.queryByText('resolved note')).not.toBeInTheDocument();
    expect(screen.getByText('No resolved comments')).toBeInTheDocument();
    expect(useReviewStore.getState().meta.comments.c2.status).toBeUndefined();
  });

  it('shows an empty message when there are no comments', () => {
    render(<CommentsPanel />);

    expect(screen.getByText('No comments')).toBeInTheDocument();
  });
});
