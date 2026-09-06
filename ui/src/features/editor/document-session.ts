import { DocumentModel, loadDocumentEngine, type DocumentBatch, type TextPoint } from './document-model';
import { DocumentOutbox, DocumentRequestError, type OutboxEntry } from './document-outbox';
import { DocumentDelivery, type DeliveryStatus } from './document-delivery';
import { documentRequest, DOCUMENT_REQUEST_TIMEOUT_MS } from './document-request';
import { DocumentPresence, type DocumentSelection } from './document-presence';
import { subscribeBrowserEvents, type BrowserEvents } from '../../lib/browser-events';

interface NativeState { base?: string[] | null; document?: { document_id: string; heads: string[] }; metadata?: Record<string, unknown>; writable?: boolean; format: 'automerge'; document_clock: string; bytes: number[] }
interface RemoteState { bytes: Uint8Array; base?: string[] | null; heads?: string[] }
export type DocumentSyncState = 'current' | 'refreshing' | 'reconnecting' | 'unavailable';
export interface SessionStatus extends DeliveryStatus { sync: DocumentSyncState; syncError?: string; readOnly: boolean }
export interface SessionTiming { requestTimeoutMs: number; retryMs: number; pollMs: number; deliveryDelayMs: number }
const defaultTiming: SessionTiming = { requestTimeoutMs: DOCUMENT_REQUEST_TIMEOUT_MS, retryMs: 3000, pollMs: 15000, deliveryDelayMs: 100 };

/** The session applies remote state to one model. Delivery owns durable saves;
 * notifications only request refreshes. Neither waits for the other network path. */
export class DocumentSession {
  private events?: BrowserEvents;
  private channel?: BroadcastChannel;
  private refreshTimer?: ReturnType<typeof setTimeout>;
  private refreshTask?: Promise<void>;
  private resetTask?: Promise<void>;
  private refreshing = false;
  private refreshAgain = false;
  private refreshState: DocumentSyncState;
  private refreshError?: string;
  private recoveryError?: string;
  private resetEpoch = 0;
  private resetting = false;
  private closed = false;
  private started = false;
  private suspended = false;
  private activeRead?: AbortController;
  private composing = false;
  private deferredRemote?: RemoteState;
  private knownHeads?: string[];
  private clock: string;
  private presence: DocumentPresence;
  private delivery: DocumentDelivery;
  private lastStatus?: string;
  readOnly: boolean;
  metadata: Record<string, unknown>;
  onPresence?: (peers: DocumentSelection[]) => void;
  onRemote?: (bytes: Uint8Array, base?: string[] | null, heads?: string[]) => void;
  onStatus?: (status: SessionStatus) => void;

  private constructor(
    readonly url: string,
    readonly documentId: string,
    readonly author: string,
    readonly actorId: string,
    readonly model: DocumentModel,
    private readonly outbox: DocumentOutbox,
    private readonly token: string | undefined,
    state: NativeState,
    entries: OutboxEntry[],
    private readonly timing: SessionTiming,
    fresh: boolean,
  ) {
    this.clock = state.document_clock; this.knownHeads = state.document?.heads;
    this.readOnly = state.writable === false; this.metadata = state.metadata ?? {};
    this.refreshState = fresh ? 'current' : 'reconnecting';
    this.presence = new DocumentPresence(this.resource('selection'), actorId, author, (peers) => this.onPresence?.(peers));
    this.delivery = new DocumentDelivery(outbox, documentId, entries, async (entry, signal) => {
      const ack = await documentRequest(this.resource(entry.kind === 'native' ? 'document-commands' : 'transactions'), entry.envelope, signal, timing.requestTimeoutMs);
      if (entry.kind === 'native' && ack.request_id !== entry.requestId) throw new Error('The server did not acknowledge this document request');
    }, () => this.publishStatus(), (event) => {
      this.channel?.postMessage(event);
      if (event === 'committed') this.requestRefresh();
    }, timing.retryMs, timing.pollMs, timing.deliveryDelayMs);
  }

  static async open(url: string, documentId: string, author: string, actorId: string, token?: string, options: Partial<SessionTiming> = {}) {
    const timing = { ...defaultTiming, ...options };
    const library = url.match(/^(.*\/v1\/libraries\/[^/]+)\/documents\//);
    if (library) url = `${library[1]}/documents-by-id/${encodeURIComponent(documentId)}`;
    await loadDocumentEngine();
    const outbox = await DocumentOutbox.open();
    let model: DocumentModel | undefined;
    try {
      let state: NativeState, fresh = true;
      try { state = await documentRequest(resourceUrl(url, 'document-state', token), undefined, undefined, timing.requestTimeoutMs); }
      catch (error) {
        if (error instanceof DocumentRequestError && error.status < 500) throw error;
        const cached = await outbox.cached(documentId);
        if (!cached) throw error;
        fresh = false;
        state = { format: 'automerge', document_clock: cached.document_clock, bytes: Array.from(cached.bytes), writable: cached.writable, metadata: cached.metadata,
          document: cached.heads ? { document_id: documentId, heads: cached.heads } : undefined };
      }
      if (state.format !== 'automerge') throw new Error('This document cannot use the editor');
      model = new DocumentModel(Uint8Array.from(state.bytes));
      if (model.view().document_id !== documentId) throw new Error('The document changed while opening');
      const entries = await outbox.list(documentId);
      for (const entry of entries.filter((entry) => entry.state !== 'archived')) {
        // Rejected structural drafts retain their full bytes for recovery.
        try { model.merge(entry.draft); } catch { /* The server decides each request. */ }
      }
      const session = new DocumentSession(url, documentId, author, actorId, model, outbox, token, state, entries, timing, fresh);
      await outbox.cache(session.snapshot());
      return session;
    } catch (error) { model?.dispose(); outbox.close(); throw error; }
  }

  private resource(suffix: string) { return resourceUrl(this.url, suffix, this.token); }
  private snapshot() {
    return { documentId: this.documentId, document_clock: this.clock, bytes: this.model.save(), heads: this.knownHeads,
      writable: !this.readOnly, metadata: this.metadata };
  }
  private publishStatus() {
    if (this.closed || !this.onStatus) return;
    const delivery = this.delivery.status;
    const status: SessionStatus = { ...delivery, sync: this.refreshState, syncError: this.refreshError, readOnly: this.readOnly,
      blocked: delivery.blocked || !!this.recoveryError || this.resetting || this.readOnly, error: delivery.error ?? this.recoveryError };
    // Do not render the editor again for unchanged persistence bookkeeping.
    const key = JSON.stringify({ ...status, failed: status.failed.map((entry) => [entry.requestId, entry.state, entry.error]) });
    if (this.lastStatus !== key) { this.lastStatus = key; this.onStatus(status); }
  }

  start() {
    if (this.started || this.closed) return;
    this.started = true;
    this.presence.start(); this.publishStatus();
    this.events = subscribeBrowserEvents(this.resource('events/stream'));
    for (const event of ['doc.changed', 'open', 'stream.lagged', 'doc.moved', 'doc.deleted', 'error']) this.events.addEventListener(event, this.requestRefresh);
    if (typeof BroadcastChannel !== 'undefined') {
      this.channel = new BroadcastChannel(`quarry-document-${this.documentId}`);
      this.channel.onmessage = ({ data }) => {
        this.delivery.wake();
        if (data === 'committed') this.requestRefresh();
      };
    }
    window.addEventListener('online', this.online);
    window.addEventListener('beforeunload', this.beforeUnload);
    window.addEventListener('pagehide', this.suspend);
    window.addEventListener('pageshow', this.resume);
    document.addEventListener('visibilitychange', this.visible);
    this.delivery.wake(); this.requestRefresh();
  }

  setBufferedEdit = (pending: boolean) => this.delivery.setBuffered(pending);
  enqueue = (batch: DocumentBatch) => {
    if (this.readOnly || this.resetting) throw new Error('The document is not editable');
    if (!batch.requests.length) return;
    const snapshot = this.snapshot();
    this.delivery.enqueue({ kind: 'native', documentId: this.documentId, requestId: batch.request_id,
      envelope: { ...batch, actor: { kind: 'browser', id: this.actorId, label: this.author } },
      draft: snapshot.bytes, heads: this.model.heads(), attempted: false, state: 'pending' }, snapshot);
  };
  private beforeUnload = (event: BeforeUnloadEvent) => {
    if (!this.delivery.needsUnloadWarning) return;
    event.preventDefault(); event.returnValue = '';
  };
  private online = () => { this.delivery.wake(); this.requestRefresh(); };
  private visible = () => { if (document.visibilityState === 'visible') this.online(); };
  private suspend = () => {
    this.suspended = true; this.activeRead?.abort(); this.delivery.suspend();
    clearTimeout(this.refreshTimer); this.refreshTimer = undefined;
  };
  private resume = () => {
    if (!this.suspended || this.closed) return;
    this.suspended = false; this.delivery.resume(); this.requestRefresh();
  };

  setComposing(composing: boolean) {
    if (this.closed) return;
    this.composing = composing;
    if (!composing && this.deferredRemote) {
      const remote = this.deferredRemote; this.deferredRemote = undefined;
      try { this.receive(remote); }
      catch (error) { this.recoveryError = String(error); this.publishStatus(); }
    }
  }
  private receive(remote: RemoteState) {
    const { bytes, base, heads } = remote;
    if (heads && this.model.containsHistory(heads)) { this.knownHeads = heads; return; }
    // The receive base advances only after the model contains that history.
    if (this.composing) { this.deferredRemote = remote; return; }
    if (this.onRemote) this.onRemote(bytes, base, heads);
    else this.model.merge(bytes, base, heads);
    this.knownHeads = heads;
  }

  private requestRefresh = () => {
    if (this.closed) return;
    this.refreshAgain = true;
    clearTimeout(this.refreshTimer); this.refreshTimer = undefined;
    if (!this.refreshing && !this.suspended && !this.resetting) this.refreshTask = this.refresh();
  };
  private async refresh() {
    this.refreshing = true;
    const epoch = this.resetEpoch;
    let failed = false;
    try {
      do {
        this.refreshAgain = false;
        this.refreshState = 'refreshing'; this.publishStatus();
        const read = new AbortController(); this.activeRead = read;
        const url = this.resource('document-state');
        let state: NativeState;
        try {
          state = await documentRequest(url + (this.knownHeads?.length ? `${url.includes('?') ? '&' : '?'}since=${encodeURIComponent(this.knownHeads.join(','))}` : ''), undefined, read.signal, this.timing.requestTimeoutMs);
        } catch (error) {
          if (read.signal.aborted) return;
          throw error;
        } finally { if (this.activeRead === read) this.activeRead = undefined; }
        if (this.closed || this.suspended || epoch !== this.resetEpoch) return;
        try {
          if (state.format !== 'automerge' || state.document && state.document.document_id !== this.documentId) throw new Error('The server returned another document');
          this.receive({ bytes: Uint8Array.from(state.bytes), base: state.base, heads: state.document?.heads });
        } catch {
          this.recoveryError = 'The document changed. Your draft is kept on this device.';
          throw new Error(this.recoveryError);
        }
        this.clock = state.document_clock;
        this.readOnly = state.writable === false; this.metadata = state.metadata ?? {};
        await this.outbox.cache(this.snapshot());
        if (this.closed || this.suspended || epoch !== this.resetEpoch) return;
        this.refreshError = undefined;
        this.refreshState = this.refreshAgain ? 'refreshing' : 'current'; this.publishStatus();
      } while (this.refreshAgain && !this.closed && !this.suspended && !this.resetting);
    } catch (error) {
      if (!this.closed && !this.suspended && epoch === this.resetEpoch) {
        failed = true;
        const refused = error instanceof DocumentRequestError && [401, 403, 404, 410].includes(error.status);
        if (refused) this.readOnly = true;
        this.refreshState = refused || this.recoveryError ? 'unavailable' : 'reconnecting';
        this.refreshError = String(error); this.publishStatus();
      }
    } finally {
      this.refreshing = false;
      if (!this.closed && !this.suspended && !this.resetting) {
        // Poll even with a healthy socket: notifications can be missed silently.
        this.refreshTimer = setTimeout(this.requestRefresh, failed ? this.timing.retryMs : this.refreshAgain ? 0 : this.timing.pollMs);
      }
    }
  }

  useSavedVersion() {
    if (this.resetting) return this.resetTask!;
    return this.resetTask = this.resetModel();
  }
  private async resetModel() {
    this.resetting = true; this.resetEpoch++; this.publishStatus();
    this.activeRead?.abort(); clearTimeout(this.refreshTimer); this.delivery.suspend();
    try {
      await this.delivery.settleWrites(); await this.refreshTask;
      const read = new AbortController(); this.activeRead = read;
      const state: NativeState = await documentRequest(this.resource('document-state'), undefined, read.signal, this.timing.requestTimeoutMs);
      if (this.closed) return;
      const entries = await this.outbox.list(this.documentId);
      if (entries.some((entry) => entry.state === 'pending')) throw new Error('Wait for the remaining edits to save before using the saved version');
      for (const entry of entries.filter((entry) => entry.state === 'failed')) await this.outbox.mark(entry, 'archived');
      if (this.closed) return;
      this.model.useSavedVersion(Uint8Array.from(state.bytes)); this.clock = state.document_clock;
      this.knownHeads = state.document?.heads; this.deferredRemote = undefined; this.recoveryError = undefined;
      this.readOnly = state.writable === false; this.metadata = state.metadata ?? {};
      await this.outbox.cache(this.snapshot());
      await this.delivery.resetErrors();
      if (!this.closed) this.onRemote?.(Uint8Array.from(state.bytes));
    } finally {
      this.activeRead = undefined; this.resetting = false;
      if (!this.closed && !this.suspended) { this.delivery.resume(); this.requestRefresh(); }
      this.publishStatus();
    }
  }

  async acceptIncomingConflict(idOfConflict: string) {
    if (this.readOnly || this.resetting) throw new Error('The document is not editable');
    await this.delivery.settleWrites();
    const remaining = await this.outbox.list(this.documentId);
    if (this.delivery.needsUnloadWarning || remaining.some((entry) => entry.state === 'pending' || entry.state === 'failed')) throw new Error('Save the current changes before deciding this conflict');
    const id = crypto.randomUUID(), snapshot = this.snapshot();
    this.delivery.enqueue({ kind: 'conflict', documentId: this.documentId, requestId: id,
      envelope: { client_tx_id: id, base_clock: this.clock, actor: { kind: 'browser', id: this.actorId, label: this.author },
        ops: [{ op: 'conflict.accept_incoming', item_id: idOfConflict }] }, draft: snapshot.bytes, state: 'pending' }, snapshot);
  }

  close() {
    this.closed = true; this.activeRead?.abort(); clearTimeout(this.refreshTimer);
    this.presence.close(); this.onPresence = undefined; this.onRemote = undefined; this.onStatus = undefined;
    this.events?.close(); this.channel?.close(); this.channel = undefined;
    window.removeEventListener('online', this.online);
    window.removeEventListener('beforeunload', this.beforeUnload);
    window.removeEventListener('pagehide', this.suspend);
    window.removeEventListener('pageshow', this.resume);
    document.removeEventListener('visibilitychange', this.visible);
    return Promise.allSettled([this.delivery.close(), this.refreshTask, this.resetTask]).then(() => this.outbox.close());
  }
  setSelection(points: TextPoint[]) { this.presence.selection(points); }
}

function resourceUrl(base: string, suffix: string, token?: string) {
  return `${base}/${suffix}${token ? `?token=${encodeURIComponent(token)}` : ''}`;
}
