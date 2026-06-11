import { render } from '@testing-library/react';
import { act } from 'react';
import { ParagraphPlugin, Plate, PlateContent, createPlateEditor } from 'platejs/react';
import { afterEach, describe, expect, it } from 'vitest';
import { reviewKit } from './review-kit';
import { useReviewStore } from '../review/review-store';
import { emptyReviewMeta } from '../review/rfm-types';

afterEach(() => {
  useReviewStore.getState().setActiveId(null);
  useReviewStore.getState().setHoverId(null);
  useReviewStore.getState().setMeta(emptyReviewMeta());
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

  it('renders a resolved comment as plain text with no highlight', () => {
    act(() => {
      useReviewStore.getState().setMeta({
        comments: { c1: { by: 'user', at: '2026-06-11T00:00:00Z', body: 'Reformat', status: 'resolved' } },
        suggestions: {},
      });
    });

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

    expect(container.querySelector('[data-comment-id="c1"]')).toBeNull();
    const highlighted = container.querySelector('.bg-warn-tint');
    expect(highlighted).toBeNull();
    expect(container).toHaveTextContent('commented');
  });

  it('restores the highlight when a resolved comment is reopened', () => {
    act(() => {
      useReviewStore.getState().setMeta({
        comments: { c1: { by: 'user', at: '2026-06-11T00:00:00Z', status: 'resolved' } },
        suggestions: {},
      });
    });

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

    expect(container.querySelector('[data-comment-id="c1"]')).toBeNull();

    act(() => {
      useReviewStore.getState().setMeta({
        comments: { c1: { by: 'user', at: '2026-06-11T00:00:00Z' } },
        suggestions: {},
      });
    });

    expect(container.querySelector('[data-comment-id="c1"]')).not.toBeNull();
  });

  it('keeps the highlight keyed to the open comment when it overlaps a resolved one', () => {
    act(() => {
      useReviewStore.getState().setMeta({
        comments: {
          done: { by: 'user', at: '2026-06-11T00:00:00Z', status: 'resolved' },
          open: { by: 'user', at: '2026-06-11T00:01:00Z' },
        },
        suggestions: {},
      });
    });

    const editor = createPlateEditor({
      plugins: [ParagraphPlugin, ...reviewKit],
      value: [
        {
          type: 'p',
          children: [{ text: 'commented', comment: true, comment_done: true, comment_open: true }],
        },
      ],
    });

    const { container } = render(
      <Plate editor={editor}>
        <PlateContent />
      </Plate>,
    );

    expect(container.querySelector('[data-comment-id="open"]')).not.toBeNull();
    expect(container.querySelector('[data-comment-id="done"]')).toBeNull();
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
