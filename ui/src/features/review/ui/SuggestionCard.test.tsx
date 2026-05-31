import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { TResolvedSuggestion } from '@platejs/suggestion';

import { useReviewStore } from '../review-store';
import { SuggestionCard } from './SuggestionCard';

describe('SuggestionCard', () => {
  beforeEach(() => {
    useReviewStore.getState().setActiveId(null);
  });

  afterEach(() => {
    useReviewStore.getState().setActiveId(null);
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

    expect(screen.getByText('Add')).toBeInTheDocument();
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

    expect(screen.getByText('Replace')).toBeInTheDocument();
    expect(screen.getByText('old')).toBeInTheDocument();
    expect(screen.getByText('new')).toBeInTheDocument();
  });
});
