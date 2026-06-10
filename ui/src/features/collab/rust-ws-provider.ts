import {
  registerProviderType,
  type ProviderConstructorProps,
  type UnifiedProvider,
} from '@platejs/yjs';
import * as decoding from 'lib0/decoding';
import { Awareness } from 'y-protocols/awareness';
import { WebsocketProvider } from 'y-websocket';
import * as Y from 'yjs';

export const RUST_WS_PROVIDER_TYPE = 'rust-ws';

// Checkpoint-ack frames (Phase 5 save state). The server broadcasts this
// custom top-level message type — outside the y-protocols range 0–3 — after
// every durable commit of the session doc, and sends the current one to each
// subscriber on join. Payload: one var-length buffer carrying the committed
// doc state as a v1-encoded Yjs snapshot. The save state compares it against
// the local doc (see save-state.ts); the server half lives in
// `crates/quarry-server/src/session.rs`.
export const MSG_QUARRY_CHECKPOINT = 113;

export interface RustWsProviderOptions {
  roomName: string;
  baseUrl?: string;
  disableBc?: boolean;
  maxBackoffTime?: number;
  params?: Record<string, string>;
  protocols?: string[];
  providerFactory?: WebsocketProviderFactory;
  resyncInterval?: number;
  token?: string;
  WebSocketPolyfill?: typeof WebSocket;
}

export interface WebsocketProviderLike {
  awareness: Awareness;
  doc: Y.Doc;
  connect: () => void;
  destroy: () => void;
  disconnect: () => void;
  messageHandlers?: WebsocketProvider['messageHandlers'];
  on: WebsocketProvider['on'];
  synced: boolean;
  wsconnected: boolean;
}

export type WebsocketProviderFactory = (
  baseUrl: string,
  roomName: string,
  doc: Y.Doc,
  options: ConstructorParameters<typeof WebsocketProvider>[3]
) => WebsocketProviderLike;

let registered = false;

export function registerRustWsProviderType() {
  if (registered) return;
  registerProviderType<RustWsProviderOptions>(RUST_WS_PROVIDER_TYPE, RustWsProviderWrapper);
  registered = true;
}

export function collabWebSocketBaseUrl(location: Pick<Location, 'host' | 'protocol'> = window.location) {
  const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${protocol}//${location.host}/v1/collab`;
}

export class RustWsProviderWrapper implements UnifiedProvider {
  private _isConnected = false;
  private _isSynced = false;
  private _lastCheckpoint: Uint8Array | null = null;
  private readonly checkpointListeners = new Set<(snapshot: Uint8Array) => void>();
  private readonly onConnect?: () => void;
  private readonly onDisconnect?: () => void;
  private readonly onError?: (error: Error) => void;
  private readonly onSyncChange?: (isSynced: boolean) => void;
  private readonly provider: WebsocketProviderLike;

  readonly type = RUST_WS_PROVIDER_TYPE;

  constructor({
    awareness,
    doc,
    onConnect,
    onDisconnect,
    onError,
    onSyncChange,
    options,
  }: ProviderConstructorProps<RustWsProviderOptions>) {
    this.onConnect = onConnect;
    this.onDisconnect = onDisconnect;
    this.onError = onError;
    this.onSyncChange = onSyncChange;

    const document = doc ?? new Y.Doc();
    const providerAwareness = awareness ?? new Awareness(document);
    const providerFactory = options.providerFactory ?? defaultProviderFactory;
    const params = { ...options.params };
    if (options.token) params.token = options.token;

    this.provider = providerFactory(options.baseUrl ?? collabWebSocketBaseUrl(), options.roomName, document, {
      awareness: providerAwareness,
      connect: false,
      disableBc: options.disableBc,
      maxBackoffTime: options.maxBackoffTime,
      params,
      protocols: options.protocols,
      resyncInterval: options.resyncInterval,
      WebSocketPolyfill: options.WebSocketPolyfill,
    });

    if (this.provider.messageHandlers) {
      this.provider.messageHandlers[MSG_QUARRY_CHECKPOINT] = (_encoder, decoder) => {
        this.receiveCheckpoint(decoding.readVarUint8Array(decoder));
      };
    }

    // One connection attempt per provider: sessions are server-seeded and a
    // Y.Doc that has fallen out of one (or bootstrapped locally while the
    // server was unreachable) must never sync back into a freshly seeded
    // room — it would merge stale content in as duplicates. The editor
    // reconnects by mounting a fresh doc + provider instead
    // (PlateMarkdownEditor). The halt hangs off 'connection-close' because
    // y-websocket emits NO status:'disconnected' for an attempt that never
    // opened — only 'connection-close' fires for every socket close, opened
    // or not. Deferred one microtask: the event fires while the provider
    // still references the closing socket, and disconnecting synchronously
    // would recurse into the same close path.
    this.provider.on('connection-close', () => {
      queueMicrotask(() => this.provider.disconnect());
    });
    this.provider.on('status', ({ status }) => {
      const wasConnected = this._isConnected;
      this._isConnected = status === 'connected';
      if (this._isConnected && !wasConnected) {
        this.onConnect?.();
      } else if (!this._isConnected && wasConnected) {
        this.onDisconnect?.();
      }
    });
    this.provider.on('sync', (isSynced) => {
      if (this._isSynced === isSynced) return;
      this._isSynced = isSynced;
      this.onSyncChange?.(isSynced);
    });
    this.provider.on('connection-error', (event) => {
      this.onError?.(event instanceof Error ? event : new Error('collab websocket connection failed'));
    });
  }

  get awareness() {
    return this.provider.awareness;
  }

  get document() {
    return this.provider.doc;
  }

  get isConnected() {
    return this._isConnected || this.provider.wsconnected;
  }

  get isSynced() {
    return this._isSynced || this.provider.synced;
  }

  /** The last checkpoint-ack snapshot received on this connection. */
  get lastCheckpoint(): Uint8Array | null {
    return this._lastCheckpoint;
  }

  /** Subscribes to checkpoint-ack frames; returns the unsubscribe. */
  onCheckpoint(listener: (snapshot: Uint8Array) => void): () => void {
    this.checkpointListeners.add(listener);
    return () => {
      this.checkpointListeners.delete(listener);
    };
  }

  private receiveCheckpoint(snapshot: Uint8Array) {
    this._lastCheckpoint = snapshot;
    for (const listener of this.checkpointListeners) {
      listener(snapshot);
    }
  }

  connect() {
    this.provider.connect();
  }

  disconnect() {
    const wasConnected = this.isConnected;
    const wasSynced = this.isSynced;
    this.provider.disconnect();
    this._isConnected = false;
    this._isSynced = false;
    if (wasConnected) this.onDisconnect?.();
    if (wasSynced) this.onSyncChange?.(false);
  }

  destroy() {
    this.provider.destroy();
    this._isConnected = false;
    this._isSynced = false;
    this.checkpointListeners.clear();
  }
}

function defaultProviderFactory(
  baseUrl: string,
  roomName: string,
  doc: Y.Doc,
  options: ConstructorParameters<typeof WebsocketProvider>[3]
) {
  return new WebsocketProvider(baseUrl, roomName, doc, options);
}
