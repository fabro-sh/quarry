import { slateNodesToInsertDelta, withYjs, YjsEditor } from '@slate-yjs/core';
import type { Value } from 'platejs';
import { createPlateEditor, ParagraphPlugin, type PlateEditor } from 'platejs/react';
import { describe, expect, it } from 'vitest';
import * as Y from 'yjs';

import { hasCommentDraft, startCommentDraft } from './comment-draft';
import { reviewKit } from '../editor/review-kit';

// Two editors bound to two Y.Docs that relay updates to each other — an
// in-memory stand-in for two browsers sharing a collab room over the rust-ws
// provider. Everything below the websocket is the real production stack:
// slate-yjs binding, Plate plugins, and the comment-draft helpers.

type CollabEditor = PlateEditor & YjsEditor;

function connectedEditorPair(value: Value): { editorA: CollabEditor; editorB: CollabEditor } {
  const docA = new Y.Doc();
  const docB = new Y.Doc();

  docA.get('content', Y.XmlText).applyDelta(slateNodesToInsertDelta(value));
  Y.applyUpdate(docB, Y.encodeStateAsUpdate(docA));

  // Live relay in both directions. Yjs emits no 'update' for a transaction
  // that changes nothing, so echoing an already-applied update terminates.
  docA.on('update', (update: Uint8Array) => Y.applyUpdate(docB, update));
  docB.on('update', (update: Uint8Array) => Y.applyUpdate(docA, update));

  return { editorA: collabEditor(docA), editorB: collabEditor(docB) };
}

function collabEditor(doc: Y.Doc): CollabEditor {
  const editor = withYjs(
    createPlateEditor({ plugins: [ParagraphPlugin, ...reviewKit] }) as never,
    doc.get('content', Y.XmlText)
  ) as unknown as CollabEditor;
  // Headless editors keep Plate's warning stub for api.redecorate (the real
  // one is installed when the editor UI mounts). There is no UI to repaint
  // here, so replace the stub to keep the warning out of test output.
  editor.api.redecorate = () => {};
  YjsEditor.connect(editor);
  return editor;
}

// Slate batches operations and flushes them (including slate-yjs's push of
// local changes into the Y.Doc) on a microtask after the transform returns.
function editsPropagated(): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, 0);
  });
}

describe('comment drafts under collaboration', () => {
  it('starting a draft on one editor does not become a draft on the peer editor', async () => {
    const { editorA, editorB } = connectedEditorPair([
      { type: 'p', children: [{ text: 'Comment this word.' }] },
    ]);

    // Sanity: the relay pipe works — a content edit by A reaches B. Without
    // this, the draft assertion below could pass vacuously on a broken pipe.
    editorA.tf.insertText('!', { at: { path: [0, 0], offset: 18 } });
    await editsPropagated();
    expect(editorB.api.string([])).toBe('Comment this word.!');

    editorA.tf.select({
      anchor: { path: [0, 0], offset: 13 },
      focus: { path: [0, 0], offset: 17 },
    });
    startCommentDraft(editorA);
    await editsPropagated();

    // The author sees their own draft...
    expect(hasCommentDraft(editorA)).toBe(true);
    // ...but it must stay private until submitted. hasCommentDraft is the
    // predicate ReviewRail uses to show the DraftCommentComposer, so a draft
    // visible here pops an empty composer in the peer's browser.
    expect(hasCommentDraft(editorB)).toBe(false);
    // The root invariant: the draft never enters the shared document at all —
    // anything in it syncs to every peer and outlives a disconnect mid-draft.
    expect(JSON.stringify(editorB.children)).not.toContain('comment_draft');
  });
});
