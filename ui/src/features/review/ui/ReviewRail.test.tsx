import { render, screen } from '@testing-library/react';
import { Plate, ParagraphPlugin, createPlateEditor, type PlateEditor } from 'platejs/react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { reviewKit } from '../../editor/review-kit';
import { addComment, useReviewStore } from '../review-store';
import { emptyReviewMeta } from '../rfm-types';
import { ReviewRail } from './ReviewRail';

// The rail mounts inside <Plate> in production (it reads the live editor via
// useEditorSelector); render it the same way here so the editor context is real.
function withPlate(editor: PlateEditor, children: ReactNode) {
  return <Plate editor={editor}>{children}</Plate>;
}

const at = '2026-01-01T00:00:00.000Z';

describe('ReviewRail', () => {
  beforeEach(() => {
    useReviewStore.getState().hydrate(emptyReviewMeta());
    useReviewStore.getState().setActiveId(null);
  });

  afterEach(() => {
    useReviewStore.getState().hydrate(emptyReviewMeta());
    useReviewStore.getState().setActiveId(null);
  });

  it('lists a comment card and a suggestion card from the editor and store', () => {
    const editor = createPlateEditor({
      plugins: [ParagraphPlugin, ...reviewKit],
      value: [
        {
          type: 'p',
          children: [
            { text: 'Plain ' },
            { text: 'noted', comment: true, comment_c1: true },
            { text: ' and ' },
            {
              text: 'inserted',
              suggestion: true,
              suggestion_s1: { id: 's1', type: 'insert', userId: 'user', createdAt: 0 },
            },
          ],
        },
      ],
    });
    useReviewStore.getState().hydrate(addComment(emptyReviewMeta(), 'c1', { by: 'user', at, body: 'note' }));

    render(withPlate(editor, <ReviewRail editor={editor} />));

    expect(screen.getByText('note')).toBeInTheDocument();
    // Both the comment author and the suggestion author render as "user".
    expect(screen.getAllByText('user').length).toBeGreaterThan(0);
    // The suggestion card shows the "Add" label and the inserted text.
    expect(screen.getByText('Add')).toBeInTheDocument();
    expect(screen.getByText('inserted')).toBeInTheDocument();
  });

  it('renders nothing when there are no comments or suggestions', () => {
    const editor = createPlateEditor({
      plugins: [ParagraphPlugin, ...reviewKit],
      value: [{ type: 'p', children: [{ text: 'Nothing to review.' }] }],
    });

    render(withPlate(editor, <ReviewRail editor={editor} />));

    expect(screen.queryByTestId('review-rail')).not.toBeInTheDocument();
  });
});
