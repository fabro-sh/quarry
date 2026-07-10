import { afterEach, describe, expect, test, vi } from 'vitest';

import {
  collabLifecycleSnapshot,
  resetCollabLifecycleCounters,
} from './collab-debug';
import { CollabEditorSession } from './collab-editor-session';

class FakeSocket {
  onclose: ((event: CloseEvent) => void) | null = null;
  onopen: ((event: Event) => void) | null = null;
  closed = false;

  close(): void {
    this.closed = true;
  }

  open(): void {
    this.onopen?.(new Event('open'));
  }

  fail(): void {
    this.onclose?.(new CloseEvent('close'));
  }
}

afterEach(() => {
  vi.useRealTimers();
  resetCollabLifecycleCounters();
});

describe('CollabEditorSession', () => {
  test('starts one probe after initialization and disposes it exactly once', () => {
    const sockets: FakeSocket[] = [];
    const session = new CollabEditorSession({
      baseUrl: 'ws://example.test/collab',
      enabled: true,
      roomName: 'doc-a',
      socketFactory: () => {
        const socket = new FakeSocket();
        sockets.push(socket);
        return socket;
      },
    });

    session.markInitialized();
    expect(sockets).toHaveLength(1);
    expect(collabLifecycleSnapshot().probe_attempted).toBe(1);

    session[Symbol.dispose]();
    session[Symbol.dispose]();
    expect(sockets[0]?.closed).toBe(true);
    expect(session.getSnapshot().lifecycle).toBe('disposed');
  });

  test('creates a fresh epoch only after a successful reachability probe', () => {
    const sockets: FakeSocket[] = [];
    const session = new CollabEditorSession({
      baseUrl: 'ws://example.test/collab',
      enabled: true,
      roomName: 'doc-a',
      socketFactory: () => {
        const socket = new FakeSocket();
        sockets.push(socket);
        return socket;
      },
    });
    session.markInitialized();

    sockets[0]?.open();

    expect(session.getSnapshot()).toMatchObject({
      epoch: 1,
      lifecycle: 'initializing',
      readOnly: true,
    });
    expect(collabLifecycleSnapshot().session_epoch_started).toBe(1);
    session[Symbol.dispose]();
  });

  test('ignores a late probe open after suspension', () => {
    const sockets: FakeSocket[] = [];
    const session = new CollabEditorSession({
      baseUrl: 'ws://example.test/collab',
      enabled: true,
      roomName: 'doc-a',
      socketFactory: () => {
        const socket = new FakeSocket();
        sockets.push(socket);
        return socket;
      },
    });
    session.markInitialized();
    const lateOpen = sockets[0]?.onopen;

    session.suspend();
    lateOpen?.(new Event('open'));

    expect(session.getSnapshot()).toMatchObject({
      epoch: 0,
      lifecycle: 'reconnecting',
    });
    expect(collabLifecycleSnapshot().session_epoch_started).toBe(0);
    session[Symbol.dispose]();
  });

  test('ignores a late probe open after the provider becomes live', () => {
    const sockets: FakeSocket[] = [];
    const session = new CollabEditorSession({
      baseUrl: 'ws://example.test/collab',
      enabled: true,
      roomName: 'doc-a',
      socketFactory: () => {
        const socket = new FakeSocket();
        sockets.push(socket);
        return socket;
      },
    });
    session.markInitialized();
    const lateOpen = sockets[0]?.onopen;

    session.observeSaveState('saved');
    lateOpen?.(new Event('open'));

    expect(session.getSnapshot()).toMatchObject({
      epoch: 0,
      lifecycle: 'live',
      readOnly: false,
      saveState: 'saved',
    });
    expect(collabLifecycleSnapshot().session_epoch_started).toBe(0);
    session[Symbol.dispose]();
  });

  test('retries a failed probe but stops retrying after the session becomes live', () => {
    vi.useFakeTimers();
    const sockets: FakeSocket[] = [];
    const session = new CollabEditorSession({
      baseUrl: 'ws://example.test/collab',
      enabled: true,
      retryMs: 25,
      roomName: 'doc-a',
      socketFactory: () => {
        const socket = new FakeSocket();
        sockets.push(socket);
        return socket;
      },
    });
    session.markInitialized();
    sockets[0]?.fail();
    vi.advanceTimersByTime(25);
    expect(sockets).toHaveLength(2);

    session.observeSaveState('saved');
    expect(sockets[1]?.closed).toBe(true);
    vi.advanceTimersByTime(100);
    expect(sockets).toHaveLength(2);
    expect(session.getSnapshot()).toMatchObject({ lifecycle: 'live', readOnly: false });
    session[Symbol.dispose]();
  });

  test('treats refusal as terminal for the document identity', () => {
    vi.useFakeTimers();
    const sockets: FakeSocket[] = [];
    const session = new CollabEditorSession({
      baseUrl: 'ws://example.test/collab',
      enabled: true,
      retryMs: 25,
      roomName: 'doc-a',
      socketFactory: () => {
        const socket = new FakeSocket();
        sockets.push(socket);
        return socket;
      },
    });
    session.markInitialized();
    session.refuse('unsupported document');
    vi.advanceTimersByTime(100);

    expect(sockets).toHaveLength(1);
    expect(sockets[0]?.closed).toBe(true);
    expect(session.getSnapshot()).toMatchObject({
      lifecycle: 'refused',
      refusalReason: 'unsupported document',
      saveState: 'refused',
    });
    session[Symbol.dispose]();
  });
});
