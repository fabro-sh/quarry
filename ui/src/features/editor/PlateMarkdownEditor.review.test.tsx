import { render, screen } from '@testing-library/react';
import { SuggestionPlugin } from '@platejs/suggestion/react';
import { slateToDeterministicYjsState } from '@platejs/yjs';
import { YjsPlugin } from '@platejs/yjs/react';
import { slateNodesToInsertDelta, yTextToSlateElement } from '@slate-yjs/core';
import { ParagraphPlugin, createPlateEditor } from 'platejs/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  collabYjsInitOptions,
  PlateMarkdownEditor,
  shouldSkipUnhydratedCollabPublish,
} from './PlateMarkdownEditor';
import { reviewKit } from './review-kit';
import { useReviewStore } from '../review/review-store';
import { currentAuthor } from '../review/identity';
import { reviewToMarkdown } from '../review/rfm-codec';
import { emptyReviewMeta } from '../review/rfm-types';
import { syncSuggestionsFromValue } from '../review/review-store';
import type { Value } from 'platejs';
import * as Y from 'yjs';

const DOC = 'See {==here==}{>>fix this<<}{#c1}.\n\n---\ncomments:\n  c1:\n    at: "2026-01-01T00:00:00.000Z"\n    by: user\n';
const DOC_B = 'A different document with no review marks.\n';
// Two distinct documents that BOTH carry review marks + endmatter. Swapping
// between them is the case that exposes the reseed bug: the baseline must be
// computed from the INCOMING doc's freshly-parsed meta, not the outgoing doc's
// store meta.
const DOC_C = 'See {==here==}{>>fix this<<}{#c1}.\n\n---\ncomments:\n  c1:\n    at: "2026-01-01T00:00:00.000Z"\n    by: user\n';
const DOC_D = 'Look {==there==}{>>change me<<}{#c9}.\n\n---\ncomments:\n  c9:\n    at: "2026-02-02T00:00:00.000Z"\n    by: user\n';

// These tests render <PlateMarkdownEditor>, which writes to the global
// useReviewStore singleton. Reset it around every test so they neither leave nor
// depend on dirty shared state. The comment-draft flow itself is covered as a
// unit in features/review/comment-draft.test.ts.
function resetReviewStore() {
  useReviewStore.getState().hydrate(emptyReviewMeta());
  useReviewStore.getState().setActiveId(null);
  useReviewStore.getState().setHoverId(null);
}

beforeEach(resetReviewStore);
afterEach(resetReviewStore);

describe('PlateMarkdownEditor link rendering', () => {
  it('renders a markdown link as an anchor carrying its href', () => {
    render(<PlateMarkdownEditor content="See [Plate](https://platejs.org)." onChange={vi.fn()} />);
    const anchor = screen.getByText('Plate').closest('a');
    expect(anchor).not.toBeNull();
    expect(anchor?.getAttribute('href')).toContain('platejs.org');
  });

  it('renders a wiki-link as a chip showing its display text', () => {
    render(<PlateMarkdownEditor content="See [[notes/guide|Guide]] now." onChange={vi.fn()} />);
    const chip = screen.getByTestId('wikilink');
    expect(chip).toHaveTextContent('Guide');
    // No resolver in this harness, so it reads as unresolved.
    expect(chip).toHaveAttribute('data-resolved', 'false');
  });

  it('renders a markdown image reference as an <img>', () => {
    render(<PlateMarkdownEditor content="![](assets/x.png)" onChange={vi.fn()} />);
    const img = document.querySelector('img');
    expect(img).not.toBeNull();
    expect(img?.getAttribute('src')).toContain('assets/x.png');
  });
});

describe('PlateMarkdownEditor review round-trip', () => {
  it('renders a commented range as a comment mark', () => {
    render(<PlateMarkdownEditor content={DOC} onChange={vi.fn()} />);
    // Plate's CommentPlugin styles a comment leaf with the `slate-comment`
    // class; the commented "here" range should carry it while plain text does
    // not.
    expect(screen.getByText('here')).toBeInTheDocument();
    const marked = document.querySelector('.slate-comment');
    expect(marked).not.toBeNull();
    expect(marked).toHaveTextContent('here');
  });

  it('does not fire onChange when the document is swapped out (pure load)', () => {
    const onChange = vi.fn();
    const { rerender } = render(<PlateMarkdownEditor content={DOC} onChange={onChange} />);
    // Loading a new document is not a user edit: hydrating the store
    // synchronously notifies the save subscription, which must short-circuit on
    // the equality guard rather than reporting a spurious change. (Before the
    // fix this fired onChange once with DOC_B's serialized text.)
    onChange.mockClear();
    rerender(<PlateMarkdownEditor content={DOC_B} onChange={onChange} />);
    expect(onChange).not.toHaveBeenCalled();
  });

  it('does not fire onChange when swapping between two docs that both carry review marks', () => {
    const onChange = vi.fn();
    const { rerender } = render(<PlateMarkdownEditor content={DOC_C} onChange={onChange} />);
    // The reseed baseline must be derived from the INCOMING doc's freshly-parsed
    // meta. If it instead serializes using the outgoing doc's store meta (which
    // `storeHydrate` only replaces on the next line), the store subscription —
    // fired synchronously inside `storeHydrate` with the new meta — diverges
    // from the stale baseline and spuriously fires onChange on a pure load.
    onChange.mockClear();
    rerender(<PlateMarkdownEditor content={DOC_D} onChange={onChange} />);
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe('PlateMarkdownEditor collaboration lifecycle', () => {
  it('syncs the provider before applying the initial markdown seed', () => {
    const options = collabYjsInitOptions('doc-collab', [{ type: 'p', children: [{ text: 'Guide' }] }]);

    expect(options.autoConnect).toBe(true);
    expect(options.onReady).toBeUndefined();
  });

  it('documents why seed-before-sync duplicates existing Yjs room content', async () => {
    const baseNodes: Value = [{ type: 'p', children: [{ text: 'hello' }] }];
    const currentNodes: Value = [
      { type: 'p', children: [{ text: 'hello' }] },
      { type: 'p', children: [{ text: 'peer' }] },
    ];
    const serverDoc = new Y.Doc();
    Y.applyUpdate(serverDoc, await slateToDeterministicYjsState('doc-collab', baseNodes));
    serverDoc.get('content', Y.XmlText).applyDelta(
      slateNodesToInsertDelta([{ type: 'p', children: [{ text: 'peer' }] }] as Value)
    );
    const roomUpdate = Y.encodeStateAsUpdate(serverDoc);

    const seedFirstClient = new Y.Doc();
    Y.applyUpdate(seedFirstClient, await slateToDeterministicYjsState('doc-collab', currentNodes));
    Y.applyUpdate(seedFirstClient, roomUpdate);
    expect(yjsChildren(seedFirstClient)).toHaveLength(4);

    const syncFirstClient = new Y.Doc();
    Y.applyUpdate(syncFirstClient, roomUpdate);
    const sharedRoot = syncFirstClient.get('content', Y.XmlText);
    if (sharedRoot.length === 0) {
      Y.applyUpdate(syncFirstClient, await slateToDeterministicYjsState('doc-collab', currentNodes));
    }
    expect(yjsChildren(syncFirstClient)).toHaveLength(2);
  });

  it('does not rebuild the Yjs plugin when equivalent collab props are recreated', () => {
    vi.useFakeTimers();
    const configure = vi.spyOn(YjsPlugin, 'configure');
    const collab = {
      documentId: 'doc-collab',
      flushAck: null,
      rebaseKey: 0,
      sessionId: 'browser:test',
      token: 'token-1',
    };

    const { rerender, unmount } = render(
      <PlateMarkdownEditor content="# Guide" collab={{ ...collab }} onChange={vi.fn()} />
    );
    const initialConfigureCalls = configure.mock.calls.length;

    rerender(<PlateMarkdownEditor content="# Guide" collab={{ ...collab }} onChange={vi.fn()} />);

    expect(configure).toHaveBeenCalledTimes(initialConfigureCalls);
    unmount();
    vi.useRealTimers();
  });

  it('publishes a blank awareness cursor name when no author is stored', () => {
    // The default 'user' sentinel is UI-only and must never reach the server:
    // blank awareness names are dropped server-side, preserving the "browser"
    // checkpoint fallback. (Review-item `by:` labels keep the 'user' default.)
    localStorage.clear();
    expect(cursorAwarenessName()).toBe('');
  });

  it('publishes the explicitly stored author as the awareness cursor name', () => {
    localStorage.setItem('quarry:author', 'Avery');
    expect(cursorAwarenessName()).toBe('Avery');
    localStorage.clear();
  });

  it('skips transient blank fallback snapshots while a collab value hydrates', () => {
    expect(shouldSkipUnhydratedCollabPublish('\n', '# Guide\n')).toBe(true);
    expect(shouldSkipUnhydratedCollabPublish('# Guide\n', '# Guide\n')).toBe(false);
    expect(shouldSkipUnhydratedCollabPublish('\n', '\n')).toBe(false);
  });
});

function yjsChildren(doc: Y.Doc) {
  return yTextToSlateElement(doc.get('content', Y.XmlText)).children;
}

// Renders a collab editor and returns the cursor name it hands to
// YjsPlugin.configure — the awareness label the server attributes live-session
// checkpoints to. The spy only observes the real call; nothing is stubbed.
function cursorAwarenessName(): unknown {
  vi.useFakeTimers();
  const configure = vi.spyOn(YjsPlugin, 'configure');
  const { unmount } = render(
    <PlateMarkdownEditor
      collab={{ documentId: 'doc-cursor', sessionId: 'browser:cursor', token: 'token-1' }}
      content="# Guide"
      onChange={vi.fn()}
    />
  );
  const config = configure.mock.calls.at(0)?.[0];
  unmount();
  configure.mockRestore();
  vi.useRealTimers();
  if (!config || typeof config === 'function') {
    throw new Error('YjsPlugin.configure was not called with a config object');
  }
  return config.options?.cursors?.data?.name;
}

// The floating toolbar only renders on selection and `usePluginOption` needs the
// live Plate context, both of which are unreliable in jsdom. So the toggle's
// behavior is exercised deterministically against a real editor built from
// `reviewKit`: the toggle does nothing more than flip `isSuggesting`, and what
// matters is that flipping it makes typing produce suggestion marks that
// round-trip to CriticMarkup. (The live button click is covered by the e2e.)
describe('Suggesting mode', () => {
  it('flips isSuggesting via the SuggestionPlugin option', () => {
    const editor = createPlateEditor({
      plugins: [ParagraphPlugin, ...reviewKit],
      value: [{ type: 'p', children: [{ text: 'hello' }] }],
    });

    expect(editor.getOption(SuggestionPlugin, 'isSuggesting')).toBe(false);
    editor.setOption(SuggestionPlugin, 'isSuggesting', true);
    expect(editor.getOption(SuggestionPlugin, 'isSuggesting')).toBe(true);
  });

  it('turns typed text into an insertion suggestion that round-trips to CriticMarkup', () => {
    const editor = createPlateEditor({
      plugins: [ParagraphPlugin, ...reviewKit],
      value: [{ type: 'p', children: [{ text: 'hello' }] }],
    });
    // Author must be set before suggesting; withSuggestion normalizes away
    // suggestion marks that lack a matching currentUserId.
    editor.setOption(SuggestionPlugin, 'currentUserId', currentAuthor());
    editor.setOption(SuggestionPlugin, 'isSuggesting', true);

    editor.tf.select({ path: [0, 0], offset: 5 });
    editor.tf.insertText('X');

    // (a) A suggestion mark now exists in the value.
    const suggested = editor.api.nodes({
      at: [],
      match: (node) => Object.keys(node).some((key) => key.startsWith('suggestion_')),
    });
    expect(Array.from(suggested)).not.toHaveLength(0);

    // (b) Serializing the value (mirroring suggestion marks into the store
    // metadata, as the editor's save path does) yields `{++X++}{#id}` plus a
    // `suggestions:` endmatter entry authored by the local user.
    const meta = syncSuggestionsFromValue(emptyReviewMeta(), editor.children);
    const markdown = reviewToMarkdown(editor.children, meta);
    expect(markdown).toMatch(/\{\+\+X\+\+\}\{#[^}]+\}/);
    expect(markdown).toContain('suggestions:');
    expect(markdown).toContain('by: user');
  });
});
