import { describe, expect, it, vi } from 'vitest';
import { Awareness } from 'y-protocols/awareness';
import * as Y from 'yjs';

import {
  RustWsProviderWrapper,
  collabWebSocketBaseUrl,
  type WebsocketProviderFactory,
  type WebsocketProviderLike,
} from './rust-ws-provider';

describe('RustWsProviderWrapper', () => {
  it('derives the collab websocket base URL from the current page protocol', () => {
    expect(collabWebSocketBaseUrl({ protocol: 'http:', host: '127.0.0.1:7831' })).toBe(
      'ws://127.0.0.1:7831/v1/collab'
    );
    expect(collabWebSocketBaseUrl({ protocol: 'https:', host: 'quarry.test' })).toBe(
      'wss://quarry.test/v1/collab'
    );
  });

  it('wraps y-websocket with document-id rooms and Plate lifecycle callbacks', () => {
    const doc = new Y.Doc();
    const awareness = new Awareness(doc);
    const fakeProvider = new FakeProvider(awareness, doc);
    const factory = vi.fn<WebsocketProviderFactory>(() => fakeProvider);
    const onConnect = vi.fn();
    const onDisconnect = vi.fn();
    const onSyncChange = vi.fn();

    const wrapper = new RustWsProviderWrapper({
      awareness,
      doc,
      onConnect,
      onDisconnect,
      onSyncChange,
      options: {
        baseUrl: 'ws://localhost:7831/v1/collab',
        providerFactory: factory,
        roomName: 'doc-123',
        token: 'invite-token',
      },
    });

    expect(factory).toHaveBeenCalledWith(
      'ws://localhost:7831/v1/collab',
      'doc-123',
      doc,
      expect.objectContaining({
        awareness,
        connect: false,
        params: { token: 'invite-token' },
      })
    );
    expect(wrapper.document).toBe(doc);
    expect(wrapper.awareness).toBe(awareness);

    wrapper.connect();
    expect(fakeProvider.connect).toHaveBeenCalledOnce();

    fakeProvider.emitStatus('connected');
    expect(wrapper.isConnected).toBe(true);
    expect(onConnect).toHaveBeenCalledOnce();

    fakeProvider.emitSync(true);
    expect(wrapper.isSynced).toBe(true);
    expect(onSyncChange).toHaveBeenCalledWith(true);

    wrapper.disconnect();
    expect(fakeProvider.disconnect).toHaveBeenCalledOnce();
    expect(wrapper.isConnected).toBe(false);
    expect(wrapper.isSynced).toBe(false);
    expect(onDisconnect).toHaveBeenCalledOnce();
    expect(onSyncChange).toHaveBeenLastCalledWith(false);
  });
});

class FakeProvider implements WebsocketProviderLike {
  connect = vi.fn();
  destroy = vi.fn();
  disconnect = vi.fn(() => {
    this.synced = false;
    this.wsconnected = false;
  });
  synced = false;
  wsconnected = false;

  private readonly handlers = new Map<string, Array<(...args: never[]) => void>>();

  constructor(
    readonly awareness: Awareness,
    readonly doc: Y.Doc
  ) {}

  on = ((event: string, handler: (...args: never[]) => void) => {
    const handlers = this.handlers.get(event) ?? [];
    handlers.push(handler);
    this.handlers.set(event, handlers);
  }) as WebsocketProviderLike['on'];

  emitStatus(status: 'connected' | 'disconnected' | 'connecting') {
    this.wsconnected = status === 'connected';
    this.emit('status', { status });
  }

  emitSync(isSynced: boolean) {
    this.synced = isSynced;
    this.emit('sync', isSynced);
  }

  private emit(event: string, ...args: unknown[]) {
    for (const handler of this.handlers.get(event) ?? []) {
      handler(...(args as never[]));
    }
  }
}
