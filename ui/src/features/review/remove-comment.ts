import { getCommentKey, isCommentNodeById } from '@platejs/comment';
import type { PlateEditor } from 'platejs/react';

// Remove the comment_<id> mark from every leaf in the document. The mark is the
// single source of truth for the warn decoration and for re-serializing the
// comment to Markdown, so deleting a comment must clear it from the editor (not
// just the review store) or the comment reappears on reload.
export function removeCommentMark(editor: PlateEditor, id: string): void {
  const key = getCommentKey(id);
  const entries = [...editor.api.nodes({ at: [], match: (node) => isCommentNodeById(node, id), mode: 'lowest' })];
  editor.tf.withoutNormalizing(() => {
    for (const [, path] of entries) {
      editor.tf.unsetNodes([key], { at: path });
    }
  });
}
