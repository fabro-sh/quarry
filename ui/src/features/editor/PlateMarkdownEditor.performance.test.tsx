import { act, render } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { emptyReviewMeta } from '../review/rfm-types';
import { useReviewStore } from '../review/review-store';
import { reviewToMarkdown } from '../review/rfm-codec';
import { PlateMarkdownEditor } from './PlateMarkdownEditor';

vi.mock('../review/rfm-codec', async () => {
  const actual = await vi.importActual<typeof import('../review/rfm-codec')>(
    '../review/rfm-codec'
  );
  return {
    ...actual,
    reviewToMarkdown: vi.fn(actual.reviewToMarkdown),
  };
});

vi.mock('../review/ui/ReviewRail', () => ({
  ReviewRail: () => null,
}));

function resetReviewStore() {
  useReviewStore.getState().hydrate(emptyReviewMeta());
  useReviewStore.getState().setActiveId(null);
  useReviewStore.getState().setHoverId(null);
}

beforeEach(() => {
  resetReviewStore();
  vi.mocked(reviewToMarkdown).mockClear();
});

afterEach(resetReviewStore);

describe('PlateMarkdownEditor serialization work', () => {
  it('does not serialize the full document when rerendering unchanged content', async () => {
    const { rerender } = render(
      <PlateMarkdownEditor content="# Guide\n" mode="editing" onChange={vi.fn()} />
    );
    // Let mount-time work settle (e.g. node-id normalization) before sampling
    // the baseline, so we measure only what the rerender itself triggers.
    await act(async () => {});
    const callsAfterMount = vi.mocked(reviewToMarkdown).mock.calls.length;

    rerender(<PlateMarkdownEditor content="# Guide\n" mode="editing" onChange={vi.fn()} />);
    await act(async () => {});

    expect(vi.mocked(reviewToMarkdown)).toHaveBeenCalledTimes(callsAfterMount);
  });
});
