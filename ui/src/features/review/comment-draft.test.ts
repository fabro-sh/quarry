import { getDraftCommentKey, isCommentKey } from '@platejs/comment';
import { ParagraphPlugin, createPlateEditor } from 'platejs/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import {
  cancelCommentDraft,
  commitCommentDraft,
  draftAnchorText,
  hasCommentDraft,
  startCommentDraft,
} from './comment-draft';
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
  // Headless editors keep Plate's warning stub for api.redecorate (the real
  // one is installed when the editor UI mounts). There is no UI to repaint
  // here, so replace the stub to keep the warning out of test output.
  editor.api.redecorate = () => {};
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

// The concatenated text of every node carrying the given comment mark.
function commentText(editor: ReturnType<typeof editorWithSelection>, id: string): string {
  return Array.from(editor.api.nodes({ at: [], match: (node) => node[id] === true }))
    .map(([node]) => ('text' in node ? node.text : ''))
    .join('');
}

describe('startCommentDraft', () => {
  it('tracks the selection as a draft without touching the document', () => {
    const editor = editorWithSelection();

    startCommentDraft(editor);

    // The draft is live and anchored to the selected word...
    expect(hasCommentDraft(editor)).toBe(true);
    expect(draftAnchorText(editor)).toBe('word');
    // ...but it is client-local state: the document carries no draft mark (a
    // mark would sync to every collaborator over Yjs), no real comment mark
    // exists, and nothing is recorded in the store.
    expect(JSON.stringify(editor.children)).not.toContain('comment_draft');
    expect(commentIds(editor)).toHaveLength(0);
    expect(Object.keys(useReviewStore.getState().getMeta().comments)).toHaveLength(0);
  });

  it('keeps the draft anchored to its words as the document changes around them', () => {
    const editor = editorWithSelection();
    startCommentDraft(editor);

    editor.tf.insertText('Please ', { at: { path: [0, 0], offset: 0 } });

    expect(draftAnchorText(editor)).toBe('word');
    commitCommentDraft(editor, 'hello');
    expect(commentText(editor, commentIds(editor)[0])).toBe('word');
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
