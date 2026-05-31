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
});
