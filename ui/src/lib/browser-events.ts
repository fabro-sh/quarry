/** Notifications must not occupy the HTTP connections used for saving.
 * Keep the EventTarget interface while using a separate WebSocket connection.
 * Consumers refresh authoritative state after reconnects and while unavailable.
 */
class BrowserEventConnection extends EventTarget {
  state: 'connecting' | 'open' | 'error' = 'connecting';
  onopen: ((event: Event) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  private socket?: WebSocket;
  private retry?: ReturnType<typeof setTimeout>;
  private timeout?: ReturnType<typeof setTimeout>;
  private closed = false;
  private suspended = false;
  private readonly address: string;

  constructor(url: string) {
    super();
    const address = new URL(url, window.location.href);
    address.protocol = address.protocol === 'https:' || address.protocol === 'wss:' ? 'wss:' : 'ws:';
    this.address = address.href;
    window.addEventListener('pagehide', this.suspend);
    window.addEventListener('pageshow', this.resume);
    // Give callers time to register handlers, including when no socket API exists.
    queueMicrotask(() => this.connect());
  }

  private emit(type: 'open' | 'error') {
    this.state = type;
    const event = new Event(type);
    if (type === 'open') this.onopen?.(event);
    else this.onerror?.(event);
    this.dispatchEvent(event);
  }

  private connect() {
    if (this.closed || this.suspended || this.socket) return;
    try {
      if (typeof WebSocket === 'undefined') { this.unavailable(); return; }
      const socket = new WebSocket(this.address);
      this.socket = socket;
      this.timeout = setTimeout(() => this.unavailable(socket), 5000);
      socket.onopen = () => {
        if (this.socket !== socket) return;
        clearTimeout(this.timeout);
        this.emit('open');
      };
      socket.onmessage = (event) => {
        if (this.socket !== socket) return;
        try {
          const payload: unknown = JSON.parse(String(event.data));
          if (typeof payload !== 'object' || !payload || !('type' in payload) || typeof payload.type !== 'string') return;
          // Only document notifications can trigger named subscribers.
          if (!/^(doc\.|directory\.|links\.|conflict\.|library\.|git\.|stream\.)/.test(payload.type)) return;
          this.dispatchEvent(new MessageEvent('notification', { data: payload }));
        } catch { /* A malformed notification never changes document state. */ }
      };
      socket.onerror = socket.onclose = () => this.unavailable(socket);
    } catch { this.unavailable(); }
  }

  private disconnect() {
    clearTimeout(this.retry); clearTimeout(this.timeout);
    const socket = this.socket; this.socket = undefined;
    if (socket) {
      socket.onopen = socket.onmessage = socket.onerror = socket.onclose = null;
      socket.close();
    }
  }

  private unavailable(socket?: WebSocket) {
    if (this.closed || this.suspended || socket && this.socket !== socket) return;
    this.disconnect();
    this.emit('error');
    if (!this.closed && !this.suspended) this.retry = setTimeout(() => this.connect(), 3000);
  }

  private suspend = () => { this.suspended = true; this.state = 'connecting'; this.disconnect(); };
  private resume = () => {
    if (!this.suspended || this.closed) return;
    this.suspended = false; this.connect();
  };

  close() {
    this.closed = true; this.disconnect();
    window.removeEventListener('pagehide', this.suspend);
    window.removeEventListener('pageshow', this.resume);
  }
}


interface SharedConnection { transport: BrowserEventConnection; users: Set<BrowserEvents> }
const connections = new Map<string, SharedConnection>();

/** Application-owned subscriptions share a scoped connection. Library document
 * interests reuse the library stream; invitation tokens retain their own scope. */
export function subscribeBrowserEvents(url: string) { return new BrowserEvents(url); }
export class BrowserEvents extends EventTarget {
  onopen: ((event: Event) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  private readonly shared: SharedConnection;
  private readonly key: string;
  private readonly documentId?: string;
  private closed = false;

  constructor(url: string) {
    super();
    const address = new URL(url, window.location.href);
    const document = address.pathname.match(/^\/v1\/libraries\/([^/]+)\/documents-by-id\/([^/]+)\/events\/stream$/);
    if (document && !address.search) {
      this.documentId = decodeURIComponent(document[2]);
      address.pathname = '/v1/events';
      address.searchParams.set('library', decodeURIComponent(document[1]));
    }
    address.protocol = address.protocol === 'https:' || address.protocol === 'wss:' ? 'wss:' : 'ws:';
    address.searchParams.sort();
    this.key = address.href;
    let shared = connections.get(this.key);
    if (!shared) {
      shared = { transport: new BrowserEventConnection(this.key), users: new Set() };
      connections.set(this.key, shared);
    }
    this.shared = shared; shared.users.add(this);
    for (const type of ['open', 'error', 'notification']) shared.transport.addEventListener(type, this.handle);
    // A new consumer must observe an already-connected or unavailable stream.
    const state = shared.transport.state;
    if (state !== 'connecting') queueMicrotask(() => { if (!this.closed && shared.transport.state === state) this.handle(new Event(state)); });
  }

  private handle = (event: Event) => {
    if (this.closed) return;
    if (event.type === 'notification') {
      const payload = (event as MessageEvent).data as { type: string; doc_id?: string };
      if (this.documentId && payload.doc_id !== this.documentId && payload.type !== 'stream.lagged') return;
      this.dispatchEvent(new MessageEvent(payload.type, { data: JSON.stringify(payload) }));
    } else {
      const notification = new Event(event.type);
      if (event.type === 'open') this.onopen?.(notification);
      else this.onerror?.(notification);
      this.dispatchEvent(notification);
    }
  };
  close() {
    if (this.closed) return;
    this.closed = true;
    for (const type of ['open', 'error', 'notification']) this.shared.transport.removeEventListener(type, this.handle);
    this.shared.users.delete(this);
    if (!this.shared.users.size) { this.shared.transport.close(); connections.delete(this.key); }
  }
}
