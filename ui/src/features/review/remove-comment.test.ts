import { ParagraphPlugin, createPlateEditor } from 'platejs/react';
import { describe, expect, it } from 'vitest';

import { reviewKit } from '../editor/review-kit';
import { removeCommentMark } from './remove-comment';

function makeEditor() {
  return createPlateEditor({
    plugins: [ParagraphPlugin, ...reviewKit],
    value: [{ type: 'p', children: [{ text: 'see ' }, { text: 'here', comment: true, comment_c1: true }, { text: '.' }] }],
  });
}

describe('removeCommentMark', () => {
  it('removes the comment_<id> mark from every leaf in the document', () => {
    const editor = makeEditor();
    removeCommentMark(editor, 'c1');
    const json = JSON.stringify(editor.children);
    expect(json).not.toContain('comment_c1');
  });
});
