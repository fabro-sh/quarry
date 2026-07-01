import * as decoding from 'lib0/decoding';
import * as encoding from 'lib0/encoding';
import { describe, expect, it, vi } from 'vitest';
import { Awareness } from 'y-protocols/awareness';
import * as Y from 'yjs';

import {
  MSG_QUARRY_CHECKPOINT,
  RustWsProviderWrapper,
  collabWebSocketBaseUrl,
  tmpCollabWebSocketBaseUrl,
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
    expect(
      tmpCollabWebSocketBaseUrl('72cb58585aa73e35758bc1141f79e32e', {
        protocol: 'http:',
        host: '127.0.0.1:5173',
      })
    ).toBe('ws://127.0.0.1:5173/v1/tmp/collab/72cb58585aa73e35758bc1141f79e32e');
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

  it('forwards tmp collab base URLs and room names to y-websocket', () => {
    const doc = new Y.Doc();
    const awareness = new Awareness(doc);
    const fakeProvider = new FakeProvider(awareness, doc);
    const factory = vi.fn<WebsocketProviderFactory>(() => fakeProvider);

    new RustWsProviderWrapper({
      awareness,
      doc,
      options: {
        baseUrl: 'ws://127.0.0.1:5173/v1/tmp/collab/72cb58585aa73e35758bc1141f79e32e',
        providerFactory: factory,
        roomName: 'content',
      },
    });

    expect(factory).toHaveBeenCalledWith(
      'ws://127.0.0.1:5173/v1/tmp/collab/72cb58585aa73e35758bc1141f79e32e',
      'content',
      doc,
      expect.objectContaining({ params: {} })
    );
  });

  it('decodes checkpoint-ack frames and notifies subscribers', () => {
    const doc = new Y.Doc();
    const awareness = new Awareness(doc);
    const fakeProvider = new FakeProvider(awareness, doc);
    const wrapper = new RustWsProviderWrapper({
      awareness,
      doc,
      options: { providerFactory: () => fakeProvider, roomName: 'doc-1' },
    });

    const received: Uint8Array[] = [];
    const unsubscribe = wrapper.onCheckpoint((snapshot) => received.push(snapshot));
    expect(wrapper.lastCheckpoint).toBeNull();

    const snapshot = Y.encodeSnapshot(Y.snapshot(doc));
    fakeProvider.receiveFrame(checkpointFrame(snapshot));
    expect(wrapper.lastCheckpoint).toEqual(snapshot);
    expect(received).toEqual([snapshot]);

    unsubscribe();
    fakeProvider.receiveFrame(checkpointFrame(snapshot));
    expect(received).toHaveLength(1);
  });

  it('stops the underlying provider on disconnect instead of letting it rejoin with a stale doc', async () => {
    const doc = new Y.Doc();
    const awareness = new Awareness(doc);
    const fakeProvider = new FakeProvider(awareness, doc);
    const onDisconnect = vi.fn();
    new RustWsProviderWrapper({
      awareness,
      doc,
      onDisconnect,
      options: { providerFactory: () => fakeProvider, roomName: 'doc-1' },
    });

    fakeProvider.emitStatus('connected');
    fakeProvider.emitConnectionClose();
    fakeProvider.emitStatus('disconnected');
    await Promise.resolve();
    // y-websocket retries closed sockets with the SAME Y.Doc; the wrapper
    // must halt that (reconnects mount a fresh doc + provider instead).
    expect(fakeProvider.disconnect).toHaveBeenCalledOnce();
    expect(onDisconnect).toHaveBeenCalledOnce();
  });

  it('halts retries for a connection attempt that never opened', async () => {
    const doc = new Y.Doc();
    const awareness = new Awareness(doc);
    const fakeProvider = new FakeProvider(awareness, doc);
    new RustWsProviderWrapper({
      awareness,
      doc,
      options: { providerFactory: () => fakeProvider, roomName: 'doc-1' },
    });

    // y-websocket emits NO status:'disconnected' when the socket never
    // opened — only 'connection-close'. Without halting here, a refused
    // connect retries forever with a stale bootstrap-seeded doc and merges
    // it into the recovered session as duplicated content.
    fakeProvider.emitConnectionClose();
    await Promise.resolve();
    expect(fakeProvider.disconnect).toHaveBeenCalledOnce();
  });
});

function checkpointFrame(snapshot: Uint8Array): Uint8Array {
  const encoder = encoding.createEncoder();
  encoding.writeVarUint(encoder, MSG_QUARRY_CHECKPOINT);
  encoding.writeVarUint8Array(encoder, snapshot);
  return encoding.toUint8Array(encoder);
}

class FakeProvider implements WebsocketProviderLike {
  connect = vi.fn();
  destroy = vi.fn();
  disconnect = vi.fn(() => {
    this.synced = false;
    this.wsconnected = false;
  });
  messageHandlers: NonNullable<WebsocketProviderLike['messageHandlers']> = [];
  synced = false;
  wsconnected = false;

  private readonly handlers = new Map<string, Array<(...args: never[]) => void>>();

  constructor(
    readonly awareness: Awareness,
    readonly doc: Y.Doc
  ) {}

  /** Mirrors y-websocket's readMessage dispatch for an inbound frame. */
  receiveFrame(frame: Uint8Array) {
    const decoder = decoding.createDecoder(frame);
    const messageType = decoding.readVarUint(decoder);
    const handler = this.messageHandlers[messageType];
    if (!handler) throw new Error(`no handler for message type ${messageType}`);
    handler(encoding.createEncoder(), decoder, this as never, false, messageType);
  }

  on = ((event: string, handler: (...args: never[]) => void) => {
    const handlers = this.handlers.get(event) ?? [];
    handlers.push(handler);
    this.handlers.set(event, handlers);
  }) as WebsocketProviderLike['on'];

  emitStatus(status: 'connected' | 'disconnected' | 'connecting') {
    this.wsconnected = status === 'connected';
    this.emit('status', { status });
  }

  /** Fires for every socket close, opened or not (unlike status). */
  emitConnectionClose() {
    this.emit('connection-close', null, this as never);
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
