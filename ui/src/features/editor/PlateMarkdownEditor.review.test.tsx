import { render, screen } from '@testing-library/react';
import { SuggestionPlugin } from '@platejs/suggestion/react';
import { ParagraphPlugin, createPlateEditor } from 'platejs/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { PlateMarkdownEditor } from './PlateMarkdownEditor';
import { reviewKit } from './review-kit';
import { useReviewStore } from '../review/review-store';
import { currentAuthor } from '../review/identity';
import { reviewToMarkdown } from '../review/rfm-codec';
import { emptyReviewMeta } from '../review/rfm-types';
import { syncSuggestionsFromValue } from '../review/review-store';

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
