import { render } from '@testing-library/react';
import { act } from 'react';
import { ParagraphPlugin, Plate, PlateContent, createPlateEditor } from 'platejs/react';
import { afterEach, describe, expect, it } from 'vitest';
import { reviewKit } from './review-kit';
import { useReviewStore } from '../review/review-store';

afterEach(() => {
  useReviewStore.getState().setActiveId(null);
  useReviewStore.getState().setHoverId(null);
});

describe('CommentLeaf', () => {
  it('renders a commented leaf with its id and reflects the active comment', () => {
    const editor = createPlateEditor({
      plugins: [ParagraphPlugin, ...reviewKit],
      value: [
        {
          type: 'p',
          children: [{ text: 'commented', comment: true, comment_c1: true }],
        },
      ],
    });

    const { container } = render(
      <Plate editor={editor}>
        <PlateContent />
      </Plate>,
    );

    const leaf = container.querySelector('[data-comment-id="c1"]');
    expect(leaf).not.toBeNull();
    expect(leaf).toHaveTextContent('commented');
    expect(leaf).toHaveAttribute('data-active', 'false');

    act(() => {
      useReviewStore.getState().setActiveId('c1');
    });

    expect(container.querySelector('[data-comment-id="c1"]')).toHaveAttribute('data-active', 'true');
  });

  it('reflects the hovered comment', () => {
    const editor = createPlateEditor({
      plugins: [ParagraphPlugin, ...reviewKit],
      value: [
        {
          type: 'p',
          children: [{ text: 'commented', comment: true, comment_c1: true }],
        },
      ],
    });

    const { container } = render(
      <Plate editor={editor}>
        <PlateContent />
      </Plate>,
    );

    expect(container.querySelector('[data-comment-id="c1"]')).toHaveAttribute('data-hover', 'false');

    act(() => {
      useReviewStore.getState().setHoverId('c1');
    });

    expect(container.querySelector('[data-comment-id="c1"]')).toHaveAttribute('data-hover', 'true');
  });
});

describe('SuggestionLeaf', () => {
  function renderSuggestion() {
    const editor = createPlateEditor({
      plugins: [ParagraphPlugin, ...reviewKit],
      value: [
        {
          type: 'p',
          children: [
            {
              text: 'inserted',
              suggestion: true,
              suggestion_s1: { id: 's1', type: 'insert', userId: 'user', createdAt: 0 },
            },
          ],
        },
      ],
    });

    return render(
      <Plate editor={editor}>
        <PlateContent />
      </Plate>,
    );
  }

  it('reflects the hovered suggestion', () => {
    const { container } = renderSuggestion();

    const leaf = container.querySelector('[data-suggestion-id="s1"]');
    expect(leaf).not.toBeNull();
    expect(leaf).toHaveAttribute('data-hover', 'false');

    act(() => {
      useReviewStore.getState().setHoverId('s1');
    });

    expect(container.querySelector('[data-suggestion-id="s1"]')).toHaveAttribute('data-hover', 'true');
  });
});
