import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { TResolvedSuggestion } from '@platejs/suggestion';

import { useReviewStore } from '../review-store';
import { SuggestionCard } from './SuggestionCard';

const insert: TResolvedSuggestion = {
  keyId: 'suggestion_s1',
  suggestionId: 's1',
  type: 'insert',
  newText: 'more',
  userId: 'user',
  createdAt: new Date(0),
};

describe('SuggestionCard', () => {
  beforeEach(() => {
    useReviewStore.getState().hydrate({ comments: {}, suggestions: {} });
    useReviewStore.getState().setActiveId(null);
    useReviewStore.getState().setHoverId(null);
  });

  afterEach(() => {
    useReviewStore.getState().hydrate({ comments: {}, suggestions: {} });
    useReviewStore.getState().setActiveId(null);
    useReviewStore.getState().setHoverId(null);
  });

  it('renders an insert summary and fires accept/reject with the suggestion id', async () => {
    const suggestion: TResolvedSuggestion = {
      keyId: 'suggestion_s1',
      suggestionId: 's1',
      type: 'insert',
      newText: 'more',
      userId: 'user',
      createdAt: new Date(0),
    };
    const onAccept = vi.fn();
    const onReject = vi.fn();

    render(<SuggestionCard suggestion={suggestion} onAccept={onAccept} onReject={onReject} />);

    expect(screen.getByText('Add:')).toBeInTheDocument();
    expect(screen.getByText('more')).toBeInTheDocument();

    await userEvent.click(screen.getByTestId('rail-accept'));
    expect(onAccept).toHaveBeenCalledWith('s1');

    await userEvent.click(screen.getByTestId('rail-reject'));
    expect(onReject).toHaveBeenCalledWith('s1');
  });

  it('renders both the old and new text for a replace suggestion', () => {
    const suggestion: TResolvedSuggestion = {
      keyId: 'suggestion_s2',
      suggestionId: 's2',
      type: 'replace',
      text: 'old',
      newText: 'new',
      userId: 'user',
      createdAt: new Date(0),
    };

    render(<SuggestionCard suggestion={suggestion} onAccept={vi.fn()} onReject={vi.fn()} />);

    expect(screen.getByText('Replace:')).toBeInTheDocument();
    expect(screen.getByText('old')).toBeInTheDocument();
    expect(screen.getByText('new')).toBeInTheDocument();
  });

  it('renders the suggestion rationale from review metadata', () => {
    useReviewStore.getState().hydrate({
      comments: {},
      suggestions: {
        s1: {
          at: '2026-01-01T00:00:00.000Z',
          body: 'This wording is easier to verify.',
          by: 'AI',
        },
      },
    });

    render(<SuggestionCard suggestion={insert} onAccept={vi.fn()} onReject={vi.fn()} />);

    expect(screen.getByTestId('suggestion-body')).toHaveTextContent(
      'This wording is easier to verify.'
    );
  });

  it('renders existing replies and adds a new reply through the composer', async () => {
    useReviewStore.getState().hydrate({
      comments: {
        r1: {
          at: '2026-01-01T00:05:00.000Z',
          body: 'Why this wording?',
          by: 'reviewer',
          re: 's1',
        },
      },
      suggestions: { s1: { at: '2026-01-01T00:00:00.000Z', by: 'AI' } },
    });
    const user = userEvent.setup();

    render(<SuggestionCard suggestion={insert} onAccept={vi.fn()} onReject={vi.fn()} />);

    expect(screen.getByText('Why this wording?')).toBeInTheDocument();

    await user.click(screen.getByTestId('suggestion-card'));
    await user.type(screen.getByLabelText('Reply'), 'Because it is clearer.');
    await user.click(screen.getByRole('button', { name: 'Submit reply' }));

    const replies = Object.values(useReviewStore.getState().getMeta().comments).filter(
      (entry) => entry.re === 's1'
    );
    expect(replies.map((reply) => reply.body)).toEqual([
      'Why this wording?',
      'Because it is clearer.',
    ]);
  });

  it('reflects the active id with data-active and the active ring', () => {
    render(<SuggestionCard suggestion={insert} onAccept={vi.fn()} onReject={vi.fn()} />);
    const card = screen.getByTestId('suggestion-card');

    expect(card).toHaveAttribute('data-active', 'false');

    act(() => {
      useReviewStore.getState().setActiveId('s1');
    });

    expect(card).toHaveAttribute('data-active', 'true');
    expect(card.className).toContain('ring-accent-ring');
  });

  it('reflects the hover id with data-hover', () => {
    render(<SuggestionCard suggestion={insert} onAccept={vi.fn()} onReject={vi.fn()} />);
    const card = screen.getByTestId('suggestion-card');

    expect(card).toHaveAttribute('data-hover', 'false');

    act(() => {
      useReviewStore.getState().setHoverId('s1');
    });

    expect(card).toHaveAttribute('data-hover', 'true');
  });

  it('sets the hover id on card mouse enter and clears it on leave', () => {
    render(<SuggestionCard suggestion={insert} onAccept={vi.fn()} onReject={vi.fn()} />);
    const card = screen.getByTestId('suggestion-card');

    fireEvent.mouseEnter(card);
    expect(useReviewStore.getState().hoverId).toBe('s1');

    fireEvent.mouseLeave(card);
    expect(useReviewStore.getState().hoverId).toBeNull();
  });

  it('scrolls into view when it becomes active', () => {
    const scrollIntoView = vi.spyOn(Element.prototype, 'scrollIntoView');
    render(<SuggestionCard suggestion={insert} onAccept={vi.fn()} onReject={vi.fn()} />);

    expect(scrollIntoView).not.toHaveBeenCalled();

    act(() => {
      useReviewStore.getState().setActiveId('s1');
    });

    expect(scrollIntoView).toHaveBeenCalledWith({ block: 'nearest' });
    scrollIntoView.mockRestore();
  });
});
