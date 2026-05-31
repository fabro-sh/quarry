import { getDraftCommentKey, isCommentKey } from '@platejs/comment';
import { ParagraphPlugin, createPlateEditor } from 'platejs/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { cancelCommentDraft, commitCommentDraft, hasCommentDraft, startCommentDraft } from './comment-draft';
import { reviewKit } from '../editor/review-kit';
import { useReviewStore } from './review-store';
import { emptyReviewMeta } from './rfm-types';

// These helpers all write to the global useReviewStore singleton. Reset it
// around every test so they neither leave nor depend on dirty shared state.
function resetReviewStore() {
  useReviewStore.getState().hydrate(emptyReviewMeta());
  useReviewStore.getState().setActiveId(null);
  useReviewStore.getState().setHoverId(null);
}

beforeEach(resetReviewStore);
afterEach(resetReviewStore);

// A real editor with the review marks and a selection over the word "word"
// (offsets 13–17 of "Comment this word."), mirroring the existing comment
// selection setup. jsdom's DOM selection is unreliable, so the helpers are
// driven against an explicit editor selection rather than a toolbar click.
function editorWithSelection() {
  const editor = createPlateEditor({
    plugins: [ParagraphPlugin, ...reviewKit],
    value: [{ type: 'p', children: [{ text: 'Comment this word.' }] }],
  });
  editor.tf.select({
    anchor: { path: [0, 0], offset: 13 },
    focus: { path: [0, 0], offset: 17 },
  });
  return editor;
}

// Real comment ids present as marks in the value, excluding the transient
// `comment_draft` key (which isCommentKey also matches).
function commentIds(editor: ReturnType<typeof editorWithSelection>): string[] {
  const ids = new Set<string>();
  const draftKey = getDraftCommentKey();
  for (const [node] of editor.api.nodes({ at: [] })) {
    for (const key of Object.keys(node)) {
      if (isCommentKey(key) && key !== draftKey) ids.add(key);
    }
  }
  return Array.from(ids);
}

describe('startCommentDraft', () => {
  it('marks the selection as a draft without committing a comment', () => {
    const editor = editorWithSelection();

    startCommentDraft(editor);

    // A draft mark covers the selection...
    expect(hasCommentDraft(editor)).toBe(true);
    expect(JSON.stringify(editor.children)).toContain('comment_draft');
    // ...but no real comment mark exists and nothing is recorded in the store.
    expect(commentIds(editor)).toHaveLength(0);
    expect(Object.keys(useReviewStore.getState().getMeta().comments)).toHaveLength(0);
  });
});

describe('commitCommentDraft', () => {
  it('promotes the draft to a real comment carrying the typed body', () => {
    const editor = editorWithSelection();
    startCommentDraft(editor);

    commitCommentDraft(editor, 'hello');

    expect(hasCommentDraft(editor)).toBe(false);
    expect(commentIds(editor)).toHaveLength(1);

    const comments = useReviewStore.getState().getMeta().comments;
    const ids = Object.keys(comments);
    expect(ids).toHaveLength(1);
    expect(comments[ids[0]].body).toBe('hello');
    expect(comments[ids[0]].by).toBe('user');
  });

  it('is a no-op when the body is empty', () => {
    const editor = editorWithSelection();
    startCommentDraft(editor);

    commitCommentDraft(editor, '   ');

    // The draft survives and nothing was committed.
    expect(hasCommentDraft(editor)).toBe(true);
    expect(commentIds(editor)).toHaveLength(0);
    expect(Object.keys(useReviewStore.getState().getMeta().comments)).toHaveLength(0);
  });
});

describe('cancelCommentDraft', () => {
  it('discards the draft without committing anything', () => {
    const editor = editorWithSelection();
    startCommentDraft(editor);

    cancelCommentDraft(editor);

    expect(hasCommentDraft(editor)).toBe(false);
    expect(JSON.stringify(editor.children)).not.toContain('comment_draft');
    expect(commentIds(editor)).toHaveLength(0);
    expect(Object.keys(useReviewStore.getState().getMeta().comments)).toHaveLength(0);
  });
});
