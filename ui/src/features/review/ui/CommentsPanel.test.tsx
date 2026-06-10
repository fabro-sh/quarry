import { fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import type {
  AgentReviewComment,
  AgentReviewConflict,
  AgentReviewResponse,
  AgentReviewSuggestion,
} from '../../../api/generated/types';
import { addComment, resolveComment, useReviewStore } from '../review-store';
import { emptyReviewMeta } from '../rfm-types';
import { CommentsPanel } from './CommentsPanel';

const at = '2026-01-01T00:00:00.000Z';

function comment(id: string, body: string, status = 'open'): AgentReviewComment {
  return {
    id,
    status,
    by: 'user',
    at,
    ref: { ordinal: 0 },
    quote: 'quoted text',
    body,
    replies: [],
  };
}

function review(overrides: Partial<AgentReviewResponse> = {}): AgentReviewResponse {
  return {
    documentId: 'doc-1',
    baseToken: 'v1',
    comments: [],
    suggestions: [],
    conflicts: [],
    ...overrides,
  };
}

// One open thread and one resolved thread, the fixture most tests start from.
function openAndResolved(): AgentReviewResponse {
  return review({
    comments: [comment('c1', 'open note'), comment('c2', 'resolved note', 'resolved')],
  });
}

describe('CommentsPanel', () => {
  beforeEach(() => {
    useReviewStore.getState().hydrate(emptyReviewMeta());
  });

  afterEach(() => {
    useReviewStore.getState().hydrate(emptyReviewMeta());
  });

  it('shows both open and resolved comments under the default All filter', () => {
    render(<CommentsPanel review={openAndResolved()} />);

    expect(screen.getByText('open note')).toBeInTheDocument();
    expect(screen.getByText('resolved note')).toBeInTheDocument();
  });

  it('filters to only open comments', () => {
    render(<CommentsPanel review={openAndResolved()} />);

    fireEvent.click(screen.getByTestId('comments-filter-open'));

    expect(screen.getByText('open note')).toBeInTheDocument();
    expect(screen.queryByText('resolved note')).not.toBeInTheDocument();
  });

  it('filters to only resolved comments', () => {
    render(<CommentsPanel review={openAndResolved()} />);

    fireEvent.click(screen.getByTestId('comments-filter-resolved'));

    // Only the resolved thread remains, so its item is unique and carries the
    // Resolved badge plus the reopen affordance.
    const item = screen.getByTestId('comments-panel-item');
    expect(within(item).getByText('resolved note')).toBeInTheDocument();
    expect(within(item).getByText('Resolved')).toBeInTheDocument();
    expect(within(item).getByTestId('reopen-comment')).toBeInTheDocument();
    expect(screen.queryByText('open note')).not.toBeInTheDocument();
  });

  it('reopening a resolved comment flips it in the shared review state', () => {
    // The reopen control routes through the live session's review map (the
    // store, here, with no doc bound); the panel itself re-renders when the
    // /review projection refreshes after the checkpoint.
    let meta = emptyReviewMeta();
    meta = addComment(meta, 'c2', { by: 'user', at, body: 'resolved note' });
    meta = resolveComment(meta, 'c2');
    useReviewStore.getState().hydrate(meta);
    render(<CommentsPanel review={openAndResolved()} />);

    fireEvent.click(screen.getByTestId('comments-filter-resolved'));
    fireEvent.click(screen.getByTestId('reopen-comment'));

    expect(useReviewStore.getState().meta.comments.c2.status).toBeUndefined();
  });

  it('badges orphaned comments whose anchored text disappeared', () => {
    render(
      <CommentsPanel
        review={review({ comments: [comment('c3', 'lost my anchor', 'orphaned')] })}
      />
    );

    const item = screen.getByTestId('comments-panel-item');
    expect(item).toHaveAttribute('data-status', 'orphaned');
    expect(within(item).getByTestId('review-status-badge')).toHaveTextContent(/orphaned/i);
  });

  it('badges invalidated suggestions and shows their replacement preview', () => {
    const suggestion: AgentReviewSuggestion = {
      id: 's1',
      status: 'invalidated',
      kind: 'replace',
      by: 'agent',
      at,
      ref: { ordinal: 0 },
      quote: 'rough',
      content: 'specific',
      preview: { before: 'rough', after: 'specific' },
    };
    render(<CommentsPanel review={review({ suggestions: [suggestion] })} />);

    const item = screen.getByTestId('comments-panel-suggestion');
    expect(within(item).getByTestId('review-status-badge')).toHaveTextContent(/invalidated/i);
    expect(within(item).getByText('rough')).toBeInTheDocument();
    expect(within(item).getByText('specific')).toBeInTheDocument();
  });

  it('shows diff3 conflict review items with kept and incoming text', () => {
    const conflict: AgentReviewConflict = {
      id: 'x1',
      status: 'open',
      by: 'git:peer',
      at,
      afterBlockId: 'b1',
      baseMarkdown: 'Original paragraph.',
      incomingMarkdown: 'Incoming rewrite.',
      canonicalMarkdown: 'Canonical rewrite.',
    };
    render(<CommentsPanel review={review({ conflicts: [conflict] })} />);

    const item = screen.getByTestId('comments-panel-conflict');
    expect(within(item).getByText('Canonical rewrite.')).toBeInTheDocument();
    expect(within(item).getByText('Incoming rewrite.')).toBeInTheDocument();
    expect(within(item).getByTestId('review-status-badge')).toHaveTextContent(/conflict/i);
  });

  it('shows an empty message when there is nothing to review', () => {
    render(<CommentsPanel review={review()} />);

    expect(screen.getByText('No comments')).toBeInTheDocument();
  });
});
