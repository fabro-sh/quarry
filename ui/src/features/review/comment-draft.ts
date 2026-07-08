import { getCommentKey, getDraftCommentKey } from '@platejs/comment';
import { nanoid } from 'nanoid';
import { KEYS, RangeApi, TextApi, type RangeRef } from 'platejs';
import { createPlatePlugin, type PlateEditor } from 'platejs/react';
import { addComment } from './review-store';
import { currentAuthor } from './identity';
import { applyReviewMutation } from './review-doc';

// A comment draft is the not-yet-posted range the composer is targeting. It is
// deliberately CLIENT-LOCAL editor state — a RangeRef held in this plugin's
// options — never a mark in the document: the document is shared live over
// Yjs, so a draft mark would surface the composer in every collaborator's
// browser before anything is posted. The RangeRef keeps the range in step with
// local and remote edits, and the decoration below paints it with the same
// leaf props a draft mark used to carry, so CommentLeaf styling is unchanged.
// Submitting (commitCommentDraft) is the only step that touches the shared
// document: it applies the real `comment_<id>` mark and records the comment,
// with its typed body, in the review store.

export interface CommentDraftOptions {
  draftRef: RangeRef | null;
}

const initialOptions: CommentDraftOptions = { draftRef: null };

export const CommentDraftPlugin = createPlatePlugin({
  key: 'commentDraft',
  options: initialOptions,
  decorate: ({ editor, entry: [node, path], getOptions }) => {
    const range = getOptions().draftRef?.current;
    if (!range || !TextApi.isText(node)) return [];
    const nodeRange = editor.api.range(path);
    const overlap = nodeRange && RangeApi.intersection(range, nodeRange);
    if (!overlap) return [];
    return [{ ...overlap, [KEYS.comment]: true, [getDraftCommentKey()]: true }];
  },
  shortcuts: {
    startCommentDraft: {
      keys: 'mod+shift+m',
      handler: ({ editor }) => startCommentDraft(editor),
    },
  },
});

export function hasCommentDraft(editor: PlateEditor): boolean {
  const range = editor.getOption(CommentDraftPlugin, 'draftRef')?.current;
  return !!range && RangeApi.isExpanded(range);
}

export function draftAnchorText(editor: PlateEditor): string {
  const range = editor.getOption(CommentDraftPlugin, 'draftRef')?.current;
  return range ? editor.api.string(range) : '';
}

export function startCommentDraft(editor: PlateEditor): void {
  const selection = editor.selection;
  if (!selection || RangeApi.isCollapsed(selection)) return;
  cancelCommentDraft(editor); // enforce a single draft at a time
  // 'inward': text typed at the edges of the anchor stays outside the draft.
  setDraftRef(editor, editor.api.rangeRef(selection, { affinity: 'inward' }));
}

export function cancelCommentDraft(editor: PlateEditor): void {
  const ref = editor.getOption(CommentDraftPlugin, 'draftRef');
  if (!ref) return;
  ref.unref();
  setDraftRef(editor, null);
}

export function commitCommentDraft(editor: PlateEditor, body: string): void {
  const text = body.trim();
  if (!text) return;
  const range = editor.getOption(CommentDraftPlugin, 'draftRef')?.current;
  if (!range || RangeApi.isCollapsed(range)) return;
  const id = nanoid();
  editor.tf.setNodes(
    { [KEYS.comment]: true, [getCommentKey(id)]: true },
    { at: range, match: TextApi.isText, split: true }
  );
  cancelCommentDraft(editor);
  applyReviewMutation((meta) =>
    addComment(meta, id, { by: currentAuthor(), at: new Date().toISOString(), body: text })
  );
}

// Decorations only recompute when something forces a render pass, so every
// draft change must redecorate explicitly — starting a draft mutates no
// document content that would otherwise trigger one.
function setDraftRef(editor: PlateEditor, ref: RangeRef | null): void {
  editor.setOption(CommentDraftPlugin, 'draftRef', ref);
  editor.api.redecorate();
}
