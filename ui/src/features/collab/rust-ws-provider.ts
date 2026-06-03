import {
  registerProviderType,
  type ProviderConstructorProps,
  type UnifiedProvider,
} from '@platejs/yjs';
import { Awareness } from 'y-protocols/awareness';
import { WebsocketProvider } from 'y-websocket';
import * as Y from 'yjs';

export const RUST_WS_PROVIDER_TYPE = 'rust-ws';

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
