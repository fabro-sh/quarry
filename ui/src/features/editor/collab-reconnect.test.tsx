import { act, render } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { PlateMarkdownEditor } from './PlateMarkdownEditor';
import { useReviewStore } from '../review/review-store';
import { emptyReviewMeta } from '../review/rfm-types';

vi.mock('../review/ui/ReviewRail', () => ({
  ReviewRail: () => null,
}));

// Reconnect-lifecycle teardown (Phase 5 review, Critical): a connection
// attempt that never OPENS gets no `status: 'disconnected'` from
// y-websocket, and @platejs/yjs's destroy() skips providers that never
// connected — without the explicit halt + teardown sweep, every unmount or
// document switch during an outage leaks a zombie provider retrying forever
// with a stale bootstrap-seeded Y.Doc, which merges duplicated content into
// the freshly seeded session on recovery. These tests pin the lifecycle
// with a WebSocket stub whose connections never open.

interface RecordedSocket {
  url: string;
  readyState: number;
}

function installNeverOpeningWebSocket() {
  const sockets: RecordedSocket[] = [];
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
      sockets.push(this);
      // The attempt is refused: close without ever opening.
      window.setTimeout(() => {
        if (this.readyState !== 0) return;
        this.readyState = 3;
        this.onerror?.(new Event('error'));
        this.onclose?.({ code: 1006 });
      }, 0);
    }

    close() {
      this.readyState = 3;
    }

    send() {}
  }
  vi.stubGlobal('WebSocket', NeverOpeningWebSocket);
  return sockets;
}

function collabEditor(documentId: string) {
  return (
    <PlateMarkdownEditor
      collab={{ documentId, sessionId: 'browser:test' }}
      content={'# Offline\n\nBootstrap body.\n'}
      onChange={() => {}}
    />
  );
}

async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

describe('collab reconnect lifecycle with unreachable connections', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    useReviewStore.getState().hydrate(emptyReviewMeta());
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
    useReviewStore.getState().hydrate(emptyReviewMeta());
  });

  it('creates no new sockets and closes every old one after unmount', async () => {
    const sockets = installNeverOpeningWebSocket();
    const { unmount } = render(collabEditor('doc-a'));

    // Init (5s sync timeout) plus a few probe cycles.
    await advance(12_000);
    expect(sockets.length).toBeGreaterThan(0);

    unmount();
    const socketsAtUnmount = sockets.length;
    await advance(30_000);

    expect(sockets.length).toBe(socketsAtUnmount);
    expect(sockets.every((socket) => socket.readyState === 3)).toBe(true);
  });

  it('stops chasing the old document after switching to another', async () => {
    const sockets = installNeverOpeningWebSocket();
    const { rerender } = render(collabEditor('doc-a'));
    await advance(12_000);
    const socketsAtSwitch = sockets.length;

    rerender(collabEditor('doc-b'));
    await advance(12_000);

    const socketsAfterSwitch = sockets.slice(socketsAtSwitch);
    expect(socketsAfterSwitch.length).toBeGreaterThan(0);
    expect(socketsAfterSwitch.every((socket) => socket.url.includes('doc-b'))).toBe(true);
    expect(
      sockets
        .filter((socket) => socket.url.includes('doc-a'))
        .every((socket) => socket.readyState === 3)
    ).toBe(true);
  });
});
