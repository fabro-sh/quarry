import { act, render } from '@testing-library/react';
import { usePlateEditor, type PlateEditor } from 'platejs/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { emptyReviewMeta } from '../review/rfm-types';
import { useReviewStore } from '../review/review-store';
import { markdownToReview, reviewToMarkdown } from '../review/rfm-codec';
import { PlateMarkdownEditor } from './PlateMarkdownEditor';

vi.mock('../review/rfm-codec', async () => {
  const actual = await vi.importActual<typeof import('../review/rfm-codec')>(
    '../review/rfm-codec'
  );
  return {
    ...actual,
    markdownToReview: vi.fn(actual.markdownToReview),
    reviewToMarkdown: vi.fn(actual.reviewToMarkdown),
  };
});

// Wraps (not stubs) usePlateEditor so tests can drive typing through the
// mounted component's real editor instance.
vi.mock('platejs/react', async () => {
  const actual = await vi.importActual<typeof import('platejs/react')>('platejs/react');
  return {
    ...actual,
    usePlateEditor: vi.fn(actual.usePlateEditor),
  };
});

function mountedEditor(): PlateEditor {
  const editor = vi.mocked(usePlateEditor).mock.results.at(-1)?.value as PlateEditor | undefined;
  if (!editor) throw new Error('usePlateEditor was not called');
  return editor;
}

vi.mock('../review/ui/ReviewRail', () => ({
  ReviewRail: () => null,
}));

function resetReviewStore() {
  useReviewStore.getState().hydrate(emptyReviewMeta());
  useReviewStore.getState().setActiveId(null);
  useReviewStore.getState().setHoverId(null);
}

beforeEach(() => {
  resetReviewStore();
  vi.mocked(markdownToReview).mockClear();
  vi.mocked(reviewToMarkdown).mockClear();
});

afterEach(resetReviewStore);

describe('PlateMarkdownEditor serialization work', () => {
  it('does not serialize the full document when rerendering unchanged content', async () => {
    const { rerender } = render(
      <PlateMarkdownEditor content="# Guide\n" mode="editing" onChange={vi.fn()} />
    );
    // Let mount-time work settle (e.g. node-id normalization) before sampling
    // the baseline, so we measure only what the rerender itself triggers.
    await act(async () => {});
    const callsAfterMount = vi.mocked(reviewToMarkdown).mock.calls.length;

    rerender(<PlateMarkdownEditor content="# Guide\n" mode="editing" onChange={vi.fn()} />);
    await act(async () => {});

    expect(vi.mocked(reviewToMarkdown)).toHaveBeenCalledTimes(callsAfterMount);
  });

  it('debounces the markdown mirror instead of serializing on every keystroke', async () => {
    vi.useFakeTimers();
    try {
      const onChange = vi.fn();
      render(<PlateMarkdownEditor content="# Guide" mode="editing" onChange={onChange} />);
      await act(async () => {});
      const callsAfterMount = vi.mocked(reviewToMarkdown).mock.calls.length;

      const editor = mountedEditor();
      // Each keystroke in its own act(): separate flushes, like separate
      // browser event turns.
      await act(async () => {
        editor.tf.select({ path: [0, 0], offset: 5 });
        editor.tf.insertText('a');
      });
      await act(async () => {
        editor.tf.insertText('b');
      });
      await act(async () => {
        editor.tf.insertText('c');
      });

      // Typing must not serialize the document synchronously — that work is
      // O(document size) and runs inside the input event.
      expect(vi.mocked(reviewToMarkdown)).toHaveBeenCalledTimes(callsAfterMount);
      expect(onChange).not.toHaveBeenCalled();

      // One trailing serialization publishes the coalesced mirror.
      await act(async () => {
        vi.runAllTimers();
      });
      expect(vi.mocked(reviewToMarkdown)).toHaveBeenCalledTimes(callsAfterMount + 1);
      expect(onChange).toHaveBeenCalledTimes(1);
      expect(onChange).toHaveBeenCalledWith(expect.stringContaining('Guideabc'));
    } finally {
      vi.useRealTimers();
    }
  });

  it('does not serialize when only hover/active review state changes', async () => {
    vi.useFakeTimers();
    try {
      render(<PlateMarkdownEditor content="# Guide" mode="editing" onChange={vi.fn()} />);
      // Two drain rounds: mount normalization fires a Slate onChange whose
      // debounced publish is scheduled during the first round.
      await act(async () => {
        vi.runAllTimers();
      });
      await act(async () => {
        vi.runAllTimers();
      });
      const callsAfterMount = vi.mocked(reviewToMarkdown).mock.calls.length;

      // Hovering / selecting a comment in the review rail flips store state
      // that has nothing to do with the document body.
      act(() => {
        useReviewStore.getState().setActiveId('comment-1');
        useReviewStore.getState().setHoverId('comment-2');
      });
      await act(async () => {
        vi.runAllTimers();
      });

      expect(vi.mocked(reviewToMarkdown)).toHaveBeenCalledTimes(callsAfterMount);
    } finally {
      vi.useRealTimers();
    }
  });

  it('parses and serializes a collab document once at mount, not twice', async () => {
    vi.useFakeTimers();
    // Collab mount needs a WebSocket; connections that never open are enough
    // for init (seeding is local, only sync needs the socket).
    class NeverOpeningWebSocket {
      static readonly CONNECTING = 0;
      static readonly OPEN = 1;
      static readonly CLOSING = 2;
      static readonly CLOSED = 3;
      url: string;
      readyState = 0;
      binaryType = 'blob';
      onopen: ((event: unknown) => void) | null = null;
      onclose: ((event: unknown) => void) | null = null;
      onerror: ((event: unknown) => void) | null = null;
      onmessage: ((event: unknown) => void) | null = null;
      constructor(url: string | URL) {
        this.url = String(url);
      }
      close() {
        this.readyState = 3;
      }
      send() {}
    }
    vi.stubGlobal('WebSocket', NeverOpeningWebSocket);
    try {
      const { unmount } = render(
        <PlateMarkdownEditor
          collab={{ documentId: 'doc-mount-cost', sessionId: 'browser:perf' }}
          content={'# Mount\n\nBody text.\n'}
          mode="editing"
          onChange={vi.fn()}
        />
      );
      // Let the mount effects and the 0ms Yjs init timer run (bounded
      // advances — runAllTimers would chase the reconnect probe forever).
      await act(async () => {
        vi.advanceTimersByTime(50);
      });
      await act(async () => {
        vi.advanceTimersByTime(400);
      });

      expect(vi.mocked(markdownToReview)).toHaveBeenCalledTimes(1);
      expect(vi.mocked(reviewToMarkdown)).toHaveBeenCalledTimes(1);
      unmount();
    } finally {
      vi.unstubAllGlobals();
      vi.useRealTimers();
    }
  });
});
