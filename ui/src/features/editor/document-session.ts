import { DocumentModel, loadDocumentEngine, type DocumentBatch } from './document-model';
import { DocumentOutbox, DocumentRequestError, drainDocumentOutbox, type OutboxEntry } from './document-outbox';
import type { DocumentSaveState } from './document-status';
import { DocumentPresence, type DocumentSelection } from './document-presence';
import type { TextPoint } from './document-model';

interface NativeState { base?: string[] | null; document?: { document_id: string; heads: string[] }; metadata?: Record<string, unknown>; writable?: boolean; format: 'automerge'; document_clock: string; bytes: number[] }
interface RemoteState { bytes: Uint8Array; base?: string[] | null; heads?: string[] }
export interface SessionStatus { state: DocumentSaveState; failed: OutboxEntry[]; error?: string; blocked?: boolean; readOnly?: boolean }

async function request(url: string, body?: unknown, signal?: AbortSignal) {
  const response = await fetch(url, body === undefined ? { cache: 'no-store', signal } : {
    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body),
  });
  const payload = await response.json().catch(() => ({ message: response.statusText || 'Invalid document response' }));
  signal?.throwIfAborted();
  if (!response.ok) throw new DocumentRequestError(response.status, payload.message ?? 'The document could not be saved');
  return payload;
}

export class DocumentSession {
  private events?: EventSource;
  private channel?: BroadcastChannel;
  private retry?: ReturnType<typeof setTimeout>;
  private closed = false;
  private suspended = false;
  private activeRead?: AbortController;
  private running = false;
  private syncTask?: Promise<void>;
  private composing = false;
  private deferredRemote?: RemoteState;
  private knownHeads?: string[];
  private again = false;
  private writes: Promise<void> = Promise.resolve();
  private pendingWrites = 0;
  private failedWrites = false;
  private pendingDecision = false;
  readOnly: boolean;
  metadata: Record<string, unknown>;
  private failed: OutboxEntry[] = [];
  private latest: Uint8Array;
  private clock: string;
  private presence: DocumentPresence;
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
  ) { this.latest = Uint8Array.from(state.bytes); this.clock = state.document_clock; this.knownHeads = state.document?.heads;
        this.readOnly = state.writable === false; this.metadata = state.metadata ?? {};
        this.presence = new DocumentPresence(this.resource('selection'), actorId, author, (peers) => this.onPresence?.(peers)); }

  static async open(url: string, documentId: string, author: string, actorId: string, token?: string) {
    const library = url.match(/^(.*\/v1\/libraries\/[^/]+)\/documents\//);
    if (library) url = `${library[1]}/documents-by-id/${encodeURIComponent(documentId)}`;
    await loadDocumentEngine();
    const outbox = await DocumentOutbox.open();
    let model: DocumentModel | undefined;
    try {
      let state: NativeState;
      try { state = await request(resourceUrl(url, 'document-state', token)); }
      catch (error) {
        if (error instanceof DocumentRequestError && error.status < 500) throw error;
        const cached = await outbox.cached(documentId);
        if (!cached) throw error;
        state = { format: 'automerge', document_clock: cached.document_clock, bytes: Array.from(cached.bytes), writable: cached.writable, metadata: cached.metadata,
          document: cached.heads ? { document_id: documentId, heads: cached.heads } : undefined };
      }
      if (state.format !== 'automerge') throw new Error('This document cannot use the editor');
      model = new DocumentModel(Uint8Array.from(state.bytes));
      if (model.view().document_id !== documentId) throw new Error('The document changed while opening');
      const entries = await outbox.list(documentId);
      for (const entry of entries.filter((entry) => entry.state !== 'archived')) {
        // A rejected structural draft can conflict with current ownership.
        // Its complete bytes remain in the outbox for explicit recovery.
        try { model.merge(entry.draft); } catch { /* The server decides each persisted request. */ }
      }
      const session = new DocumentSession(url, documentId, author, actorId, model, outbox, token, state);
      session.failed = entries.filter((entry) => entry.state === 'failed');
      session.pendingDecision = entries.some((entry) => entry.kind === 'conflict' && entry.state === 'pending');
      await outbox.cache({ documentId, document_clock: state.document_clock, bytes: model.save(), heads: session.knownHeads, writable: !session.readOnly, metadata: session.metadata });
      return session;
    } catch (error) { model?.dispose(); outbox.close(); throw error; }
  }

  private resource(suffix: string) { return resourceUrl(this.url, suffix, this.token); }

  start() {
    this.presence.start();
    this.onStatus?.({ state: this.failed.length ? 'save_failed' : 'saving', failed: this.failed, blocked: this.failed.length > 0 || this.pendingDecision });
    if (typeof EventSource !== 'undefined') {
      this.events = new EventSource(this.resource("events/stream"));
      this.events.addEventListener('doc.changed', this.kick);
      this.events.addEventListener('open', this.kick);
      this.events.addEventListener('stream.lagged', this.kick);
      this.events.addEventListener('doc.moved', this.kick);
      this.events.addEventListener('doc.deleted', this.kick);
    }
    if (typeof BroadcastChannel !== 'undefined') {
      this.channel = new BroadcastChannel(`quarry-document-${this.documentId}`);
      this.channel.onmessage = this.kick;
    }
    window.addEventListener('online', this.kick);
    window.addEventListener('beforeunload', this.beforeUnload);
    window.addEventListener('pagehide', this.suspend);
    window.addEventListener('pageshow', this.resume);
    this.kick();
  }

  enqueue = (batch: DocumentBatch) => {
    if (this.readOnly) throw new Error("This invitation allows viewing");
    if (!batch.requests.length) return;
    const entry: OutboxEntry = { kind: 'native', documentId: this.documentId, requestId: batch.request_id,
      envelope: { ...batch, actor: { kind: 'browser', id: this.actorId, label: this.author } },
      draft: this.model.save(), heads: this.model.heads(), attempted: false, state: 'pending' };
    this.persist(entry);
  };

  private beforeUnload = (event: BeforeUnloadEvent) => {
    if (!this.pendingWrites && !this.failedWrites) return;
    event.preventDefault(); event.returnValue = '';
  };

  private persist(entry: OutboxEntry) {
    // These heads describe this snapshot. A remote receive may advance the
    // session while this write waits behind another IndexedDB transaction.
    const cached = { documentId: this.documentId, document_clock: this.clock, bytes: entry.draft,
      heads: this.knownHeads, writable: !this.readOnly, metadata: this.metadata };
    this.pendingWrites++;
    this.onStatus?.({ state: 'saving', failed: [], blocked: this.pendingDecision });
    // Preserve local causal order before other tabs can pick up these rows.
    this.writes = this.writes.then(async () => {
      try {
        await this.outbox.add(entry, cached);
        this.channel?.postMessage('queued');
      } catch (error) {
        this.failedWrites = true;
        this.onStatus?.({ state: 'save_failed', failed: this.failed, error: String(error), blocked: true });
      } finally { this.pendingWrites--; this.kick(); }
    });
  };

  private kick = () => {
    if (this.closed) return;
    if (this.retry) { clearTimeout(this.retry); this.retry = undefined; }
    this.again = true;
    if (!this.running && !this.suspended) this.syncTask = this.sync();
  };

  private suspend = () => {
    this.suspended = true;
    this.activeRead?.abort();
    if (this.retry) { clearTimeout(this.retry); this.retry = undefined; }
  };

  private resume = () => {
    if (!this.suspended || this.closed) return;
    this.suspended = false;
    this.kick();
  };

  setComposing(composing: boolean) {
    if (this.closed) return;
    this.composing = composing;
    if (!composing && this.deferredRemote) {
      const remote = this.deferredRemote; this.deferredRemote = undefined;
      try { this.receive(remote); }
      catch (error) { this.onStatus?.({ state: 'save_failed', failed: this.failed, error: String(error), blocked: true }); }
    }
  }

  private receive(remote: RemoteState) {
    const { bytes, base, heads } = remote;
    if (heads && this.model.containsHistory(heads)) { this.knownHeads = heads; return; }
    // Keep the base fixed until composition ends. Each deferred response then
    // includes every remote change since that base, so replacing it is safe.
    if (this.composing) { this.deferredRemote = remote; return; }
    if (this.onRemote) this.onRemote(bytes, base, heads);
    else this.model.merge(bytes, base, heads);
    this.knownHeads = heads;
  }

  private async sync() {
    this.running = true;
    try {
      do {
        this.again = false;
        await this.writes;
        if (this.closed || this.suspended) break;
        const drain = () => drainDocumentOutbox(this.outbox, this.documentId,
          async (entry) => { await request(this.resource(entry.kind === "native" ? "document-commands" : "transactions"), entry.envelope); },
          () => this.channel?.postMessage('committed'));
        if (navigator.locks) await navigator.locks.request(`quarry-save-${this.documentId}`, { ifAvailable: true }, (lock) => lock ? drain() : undefined);
        else await drain(); // Durable server receipts also make duplicate delivery safe.
        if (this.closed || this.suspended) break;
        const stateUrl = this.resource('document-state');
        const read = new AbortController();
        this.activeRead = read;
        let state: NativeState;
        try {
          state = await request(stateUrl + (this.knownHeads?.length ? `${stateUrl.includes('?') ? '&' : '?'}since=${encodeURIComponent(this.knownHeads.join(','))}` : ''), undefined, read.signal);
        } catch (error) {
          // Navigation cancels reads, but never cancels a durable command.
          // A cached page can resume before this rejection is delivered.
          if (read.signal.aborted) continue;
          throw error;
        } finally { this.activeRead = undefined; }
        if (this.closed || this.suspended) break;
        this.latest = Uint8Array.from(state.bytes); this.clock = state.document_clock;
        this.readOnly = state.writable === false; this.metadata = state.metadata ?? {};
        const entries = await this.outbox.list(this.documentId);
        if (this.closed) break;
        this.pendingDecision = entries.some((entry) => entry.kind === "conflict" && entry.state === "pending");
        const failed = entries.filter((entry) => entry.state === 'failed');
        this.failed = failed;
        try {
          if (state.document && state.document.document_id !== this.documentId) throw new Error('The server returned another document');
          this.receive({ bytes: this.latest, base: state.base, heads: state.document?.heads });
        } catch {
          this.onStatus?.({ state: 'save_failed', failed, error: 'The document changed. Your draft is kept on this device.', blocked: true });
          break;
        }
        await this.outbox.cache({ documentId: this.documentId, document_clock: this.clock, bytes: this.model.save(), heads: this.knownHeads, writable: !this.readOnly, metadata: this.metadata });
        if (this.closed) break;
        // A local write can finish while cache() is awaiting IndexedDB. The
        // entries above then describe the previous drain, even though there
        // are no pendingWrites left. Check the requested follow-up cycle too.
        this.onStatus?.({ state: failed.length || this.failedWrites ? 'save_failed'
          : entries.some((entry) => entry.state === 'pending') || this.pendingWrites || this.again ? 'saving' : 'saved', failed, blocked: failed.length > 0 || this.failedWrites || this.pendingDecision });
      } while (this.again && !this.closed && !this.suspended);
    } catch (error) {
      if (!this.closed && !this.suspended) {
        this.onStatus?.({ state: 'save_failed', failed: this.failed, error: String(error), blocked: this.failed.length > 0 || this.failedWrites || this.pendingDecision });
        this.retry = setTimeout(this.kick, 3000);
      }
    } finally { this.running = false; }
  }

  async useSavedVersion() {
    await this.writes;
    for (const entry of await this.outbox.list(this.documentId)) {
      if (entry.state === 'failed') await this.outbox.mark(entry, 'archived');
    }
    const state: NativeState = await request(this.resource("document-state"));
    this.latest = Uint8Array.from(state.bytes);
    this.clock = state.document_clock;
    this.model.useSavedVersion(this.latest);
    this.knownHeads = state.document?.heads;
    this.deferredRemote = undefined;
    this.failedWrites = false;
    this.onRemote?.(this.latest);
    this.kick();
  }

  async acceptIncomingConflict(idOfConflict: string) {
    if (this.readOnly) throw new Error("This invitation allows viewing");
    await this.writes;
    const remaining = await this.outbox.list(this.documentId);
    if (remaining.some((entry) => entry.state === 'pending' || entry.state === 'failed')) throw new Error('Save the current changes before deciding this conflict');
    const id = crypto.randomUUID();
    this.pendingDecision = true;
    this.persist({ kind: 'conflict', documentId: this.documentId, requestId: id,
      envelope: { client_tx_id: id, base_clock: this.clock, actor: { kind: 'browser', id: this.actorId, label: this.author },
        ops: [{ op: 'conflict.accept_incoming', item_id: idOfConflict }] }, draft: this.model.save(), state: 'pending' });
  }

  close() {
    this.closed = true;
    this.activeRead?.abort();
    this.presence.close(); this.onPresence = undefined;
    this.onRemote = undefined; this.onStatus = undefined;
    this.events?.close(); this.channel?.close();
    if (this.retry) clearTimeout(this.retry);
    window.removeEventListener('online', this.kick);
    window.removeEventListener('beforeunload', this.beforeUnload);
    window.removeEventListener('pagehide', this.suspend);
    window.removeEventListener('pageshow', this.resume);
    // Pending writes retain their own snapshots and finish before closing IDB.
    return Promise.allSettled([this.writes, this.syncTask]).then(() => this.outbox.close());
  }
  setSelection(points: TextPoint[]) { this.presence.selection(points); }
}

function resourceUrl(base: string, suffix: string, token?: string) {
  return `${base}/${suffix}${token ? `?token=${encodeURIComponent(token)}` : ''}`;
}
