import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

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

function comment(
  id: string,
  body: string,
  status = 'open',
  editedAt: string | null = null
): AgentReviewComment {
  return {
    id,
    status,
    by: 'user',
    at,
    editedAt,
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

function conflictItem(status = 'open'): AgentReviewConflict {
  return {
    id: 'x1',
    status,
    by: 'git:peer',
    at,
    afterBlockId: 'b1',
    baseMarkdown: 'Original paragraph.',
    incomingMarkdown: 'Incoming rewrite.',
    canonicalMarkdown: 'Canonical rewrite.',
  };
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

  it('shows edited indicators for edited comments and replies', () => {
    render(
      <CommentsPanel
        review={review({
          comments: [
            {
              ...comment('c1', 'edited note', 'open', '2026-01-01T00:03:00.000Z'),
              replies: [
                {
                  id: 'r1',
                  status: 'open',
                  by: 'agent',
                  at,
                  editedAt: '2026-01-01T00:04:00.000Z',
                  body: 'edited reply',
                },
              ],
            },
          ],
        })}
      />
    );

    expect(screen.getAllByText('edited')).toHaveLength(2);
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
      body: 'Prefer concrete wording.',
      preview: { before: 'rough', after: 'specific' },
      replies: [],
    };
    render(<CommentsPanel review={review({ suggestions: [suggestion] })} />);

    const item = screen.getByTestId('comments-panel-suggestion');
    expect(within(item).getByTestId('review-status-badge')).toHaveTextContent(/invalidated/i);
    expect(within(item).getByText('rough')).toBeInTheDocument();
    expect(within(item).getByText('specific')).toBeInTheDocument();
    expect(within(item).getByText('Prefer concrete wording.')).toBeInTheDocument();
  });

  it('shows and resolves an open block-deletion suggestion', async () => {
    const onResolveSuggestion = vi.fn().mockResolvedValue(undefined);
    const suggestion: AgentReviewSuggestion = {
      id: 's-delete',
      status: 'open',
      kind: 'block_delete',
      by: 'agent',
      at,
      ref: { ordinal: 0 },
      quote: 'Obsolete heading',
      content: '',
      body: 'The section is no longer needed.',
      preview: { before: 'Obsolete heading', after: '' },
      replies: [
        {
          id: 'r1',
          status: 'open',
          by: 'reviewer',
          at,
          editedAt: null,
          body: 'Agreed.',
        },
      ],
    };
    render(
      <CommentsPanel
        onResolveSuggestion={onResolveSuggestion}
        review={review({ suggestions: [suggestion] })}
      />
    );

    const item = screen.getByTestId('comments-panel-suggestion');
    expect(within(item).getByText('Delete block:')).toBeInTheDocument();
    expect(within(item).getByText('The section is no longer needed.')).toBeInTheDocument();
    expect(within(item).getByText('Agreed.')).toBeInTheDocument();

    fireEvent.click(within(item).getByTestId('accept-block-delete-suggestion'));

    expect(onResolveSuggestion).toHaveBeenCalledWith('s-delete', 'accept');
    await waitFor(() =>
      expect(within(item).getByTestId('accept-block-delete-suggestion')).toBeEnabled()
    );
  });

  it('shows diff3 conflict review items with kept and incoming text', () => {
    render(<CommentsPanel review={review({ conflicts: [conflictItem()] })} />);

    const item = screen.getByTestId('comments-panel-conflict');
    expect(within(item).getByText('Canonical rewrite.')).toBeInTheDocument();
    expect(within(item).getByText('Incoming rewrite.')).toBeInTheDocument();
    expect(within(item).getByTestId('review-status-badge')).toHaveTextContent(/conflict/i);
  });

  it('dismissing an open conflict calls the handler with the conflict id', async () => {
    const onDismissConflict = vi.fn().mockResolvedValue(undefined);
    render(
      <CommentsPanel
        onDismissConflict={onDismissConflict}
        review={review({ conflicts: [conflictItem()] })}
      />
    );

    fireEvent.click(screen.getByTestId('dismiss-conflict'));

    expect(onDismissConflict).toHaveBeenCalledWith('x1');
    // The button disables while the dismissal is in flight and re-enables
    // after it settles; waiting keeps the state update inside act().
    await waitFor(() => expect(screen.getByTestId('dismiss-conflict')).toBeEnabled());
  });

  it('hides conflict actions once the conflict is resolved', () => {
    const onDismissConflict = vi.fn().mockResolvedValue(undefined);
    render(
      <CommentsPanel
        onDismissConflict={onDismissConflict}
        review={review({ conflicts: [conflictItem('resolved')] })}
      />
    );

    expect(screen.queryByTestId('dismiss-conflict')).not.toBeInTheDocument();
    expect(screen.queryByTestId('copy-conflict-incoming')).not.toBeInTheDocument();
  });

  it('copying an open conflict puts the incoming markdown on the clipboard', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    });
    try {
      render(<CommentsPanel review={review({ conflicts: [conflictItem()] })} />);

      fireEvent.click(screen.getByTestId('copy-conflict-incoming'));

      expect(writeText).toHaveBeenCalledWith('Incoming rewrite.');
      await waitFor(() =>
        expect(screen.getByTestId('copy-conflict-incoming')).toHaveTextContent('Copied')
      );
    } finally {
      Reflect.deleteProperty(navigator, 'clipboard');
    }
  });

  it('shows an empty message when there is nothing to review', () => {
    render(<CommentsPanel review={review()} />);

    expect(screen.getByText('No comments')).toBeInTheDocument();
  });
});
