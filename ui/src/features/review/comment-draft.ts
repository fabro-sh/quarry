import { getCommentKey, getDraftCommentKey } from '@platejs/comment';
import { CommentPlugin } from '@platejs/comment/react';
import { nanoid } from 'nanoid';
import type { PlateEditor } from 'platejs/react';
import { addComment } from './review-store';
import { currentAuthor } from './identity';
import { applyReviewMutation } from './review-doc';

// A comment draft is a transient `comment_draft` mark on the selected text. The
// RFM codec ignores `comment_draft`, so a draft never serializes to Markdown and
// never triggers a content save — nothing is persisted until the user submits.
// Submitting (commitCommentDraft) promotes the draft to a real `comment_<id>`
// mark and records the comment, with its typed body, in the review store.

export function hasCommentDraft(editor: PlateEditor): boolean {
  return editor.getApi(CommentPlugin).comment.nodes({ at: [], isDraft: true }).length > 0;
}

export function draftAnchorText(editor: PlateEditor): string {
  return editor
    .getApi(CommentPlugin)
    .comment.nodes({ at: [], isDraft: true })
    .map(([node]) => node.text)
    .join('');
}

export function cancelCommentDraft(editor: PlateEditor): void {
  const drafts = editor.getApi(CommentPlugin).comment.nodes({ at: [], isDraft: true });
  if (drafts.length === 0) return;
  editor.tf.withoutNormalizing(() => {
    for (const [, path] of drafts) {
      editor.tf.unsetNodes([getDraftCommentKey()], { at: path });
    }
  });
}

export function startCommentDraft(editor: PlateEditor): void {
  cancelCommentDraft(editor); // enforce a single draft at a time
  editor.getTransforms(CommentPlugin).comment.setDraft();
}

export function commitCommentDraft(editor: PlateEditor, body: string): void {
  const text = body.trim();
  if (!text) return;
  const drafts = editor.getApi(CommentPlugin).comment.nodes({ at: [], isDraft: true });
  if (drafts.length === 0) return;
  const id = nanoid();
  editor.tf.withoutNormalizing(() => {
    for (const [, path] of drafts) {
      editor.tf.setNodes({ [getCommentKey(id)]: true }, { at: path, split: true });
      editor.tf.unsetNodes([getDraftCommentKey()], { at: path });
    }
  });
  applyReviewMutation((meta) =>
    addComment(meta, id, { by: currentAuthor(), at: new Date().toISOString(), body: text })
  );
}
