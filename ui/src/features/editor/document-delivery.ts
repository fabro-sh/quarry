import { DocumentOutbox, drainDocumentOutbox, type CachedDocument, type OutboxEntry } from './document-outbox';
import type { DocumentSaveState } from './document-status';

export interface DeliveryStatus {
  state: DocumentSaveState;
  failed: OutboxEntry[];
  blocked: boolean;
  error?: string;
}

/** Owns local persistence and ordered delivery. Reads and notification health
 * never gate this worker or determine whether an edit has been saved. */
export class DocumentDelivery {
  private entries: OutboxEntry[];
  private writes: Promise<void> = Promise.resolve();
  private task?: Promise<void>;
  private attempt?: AbortController;
  private timer?: ReturnType<typeof setTimeout>;
  private running = false;
  private again = false;
  private closed = false;
  private suspended = false;
  private buffered = false;
  private pendingWrites = 0;
  private revision = 0;
  private storageError?: string;
  private deliveryError?: string;
  private backingOff = false;

  constructor(
    private readonly outbox: DocumentOutbox,
    private readonly documentId: string,
    entries: OutboxEntry[],
    private readonly send: (entry: OutboxEntry, signal: AbortSignal) => Promise<void>,
    private readonly changed: (status: DeliveryStatus) => void,
    private readonly broadcast: (event: 'queued' | 'committed') => void,
    private readonly retryMs = 3000,
    private readonly pollMs = 15000,
    private readonly deliveryDelayMs = 100,
  ) { this.entries = entries; }

  get status(): DeliveryStatus {
    const failed = this.entries.filter((entry) => entry.state === 'failed');
    const pending = this.pendingWrites > 0 || this.buffered || this.entries.some((entry) => entry.state === 'pending');
    const blocked = !!this.storageError || failed.length > 0 || this.entries.some((entry) => entry.kind === 'conflict' && entry.state === 'pending');
    return {
      state: failed.length || this.storageError || pending && this.deliveryError ? 'save_failed' : pending ? 'saving' : 'saved',
      failed, blocked, error: this.storageError,
    };
  }
  get needsUnloadWarning() { return this.buffered || this.pendingWrites > 0 || !!this.storageError; }
  setBuffered(value: boolean) { this.buffered = value; this.publish(); }
  private publish() { if (!this.closed) this.changed(this.status); }

  enqueue(entry: OutboxEntry, cached: CachedDocument) {
    if (this.closed) throw new Error('The document is closed');
    this.pendingWrites++; this.revision++;
    if (entry.kind === 'conflict') this.entries = [...this.entries, entry];
    this.publish();
    this.writes = this.writes.then(async () => {
      try {
        await this.outbox.add(entry, cached);
        this.entries = [...this.entries.filter((item) => item.requestId !== entry.requestId), entry];
        this.broadcast('queued');
      } catch (error) { this.storageError = String(error); }
      finally { this.pendingWrites--; this.revision++; this.publish(); this.kick(); }
    });
  }

  /** Ordinary invalidations preserve retry delay; online/resume can retry now. */
  kick = () => {
    if (this.closed) return;
    this.again = true;
    if (!this.running && !this.suspended && !this.backingOff) {
      if (this.timer) clearTimeout(this.timer);
      // Bound batching delay from the first edit; typing must not postpone it.
      this.backingOff = true;
      this.timer = setTimeout(() => { this.timer = undefined; this.backingOff = false; this.task = this.run(); }, this.deliveryDelayMs);
    }
  };
  wake = () => {
    clearTimeout(this.timer); this.timer = undefined; this.backingOff = false;
    this.again = true;
    if (!this.closed && !this.suspended && !this.running) this.task = this.run();
  };

  private async readStatus() {
    const revision = this.revision;
    const entries = await this.outbox.list(this.documentId);
    // A persistence callback may complete between the IDB read and this turn.
    if (revision !== this.revision) { this.again = true; return; }
    this.entries = entries;
    if (!entries.some((entry) => entry.state === 'pending')) this.deliveryError = undefined;
    this.publish();
  }

  private async run() {
    this.running = true;
    const attempt = new AbortController(); this.attempt = attempt;
    let delayed = false;
    try {
      {
        this.again = false;
        await this.writes;
        attempt.signal.throwIfAborted();
        const drain = () => drainDocumentOutbox(this.outbox, this.documentId,
          (entry) => this.send(entry, attempt.signal), async () => {
            this.deliveryError = undefined;
            await this.readStatus();
            this.broadcast('committed');
          }, attempt.signal);
        if (navigator.locks) {
          const acquired = await navigator.locks.request(`quarry-save-${this.documentId}`, { ifAvailable: true }, async (lock) => {
            if (!lock) return false;
            await drain(); return true;
          });
          delayed = !acquired;
        } else await drain(); // Exact durable receipts also guard duplicate sends.
        await this.readStatus();
      }
    } catch (error) {
      if (!attempt.signal.aborted) {
        this.deliveryError = String(error); delayed = true;
        try { await this.readStatus(); }
        catch (error) { this.storageError = String(error); }
        this.publish();
      }
    } finally {
      this.attempt = undefined; this.running = false;
      if (!this.closed && !this.suspended) {
        const pending = this.entries.some((entry) => entry.state === 'pending');
        // A finite pass releases the cross-tab lock. Polling also recovers a
        // missed BroadcastChannel message or a closed delivering tab.
        this.backingOff = delayed || pending || this.again;
        this.timer = setTimeout(() => { this.timer = undefined; this.backingOff = false; this.task = this.run(); }, delayed ? this.retryMs : pending || this.again ? this.deliveryDelayMs : this.pollMs);
      }
    }
  }

  async settleWrites() { await this.writes; }
  async resetErrors() {
    await this.writes;
    this.storageError = this.deliveryError = undefined;
    await this.readStatus(); this.wake();
  }
  suspend() {
    this.suspended = true; clearTimeout(this.timer); this.timer = undefined;
    this.backingOff = false;
    this.attempt?.abort();
  }
  resume() { this.suspended = false; this.wake(); }
  close() {
    this.closed = true; this.suspend();
    return Promise.allSettled([this.writes, this.task]);
  }
}
