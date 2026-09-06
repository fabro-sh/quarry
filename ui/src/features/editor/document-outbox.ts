import type { DocumentBatch } from './document-model';

export interface NativeEnvelope extends DocumentBatch {
  actor: { kind: 'browser'; id: string; label: string };
}
interface OutboxRecord {
  sequence?: number;
  attempted?: boolean;
  heads?: string[];
  documentId: string;
  requestId: string;
  draft: Uint8Array;
  state: 'pending' | 'failed' | 'archived';
  error?: string;
}
interface ConflictEnvelope {
  client_tx_id: string;
  base_clock: string;
  actor: NativeEnvelope['actor'];
  ops: [{ op: 'conflict.accept_incoming'; item_id: string }];
}
export type OutboxEntry = OutboxRecord & (
  { kind: 'native'; envelope: NativeEnvelope } |
  { kind: 'conflict'; envelope: ConflictEnvelope }
);
export interface CachedDocument { heads?: string[]; metadata?: Record<string, unknown>; writable?: boolean; documentId: string; document_clock: string; bytes: Uint8Array }

/** Each request has its own row. Tabs cannot overwrite one another's drafts. */
export class DocumentOutbox {
  private constructor(private readonly database: IDBDatabase) {}
  static open(name = 'quarry-document-outbox'): Promise<DocumentOutbox> {
    return new Promise((resolve, reject) => {
      const request = indexedDB.open(name, 2);
      request.onupgradeneeded = () => {
        if (!request.result.objectStoreNames.contains('requests')) {
          const store = request.result.createObjectStore('requests', { keyPath: 'sequence', autoIncrement: true });
          store.createIndex('document', 'documentId');
          store.createIndex('request', ['documentId', 'requestId'], { unique: true });
        }
        if (!request.result.objectStoreNames.contains('documents')) request.result.createObjectStore('documents', { keyPath: 'documentId' });
      };
      request.onsuccess = () => {
        request.result.onversionchange = () => request.result.close();
        resolve(new DocumentOutbox(request.result));
      };
      request.onerror = () => reject(request.error);
      request.onblocked = () => reject(new Error('Another tab is blocking draft storage'));
    });
  }
  close() { this.database.close(); }
  async add(entry: OutboxEntry, cached?: CachedDocument) {
    await this.write((store, transaction) => {
      store.add(entry);
      if (cached) transaction.objectStore('documents').put(cached);
    });
  }
  async cache(document: CachedDocument) { await this.write((_store, transaction) => transaction.objectStore('documents').put(document)); }
  cached(documentId: string): Promise<CachedDocument | undefined> {
    return new Promise((resolve, reject) => {
      const transaction = this.database.transaction('documents', 'readonly');
      const request = transaction.objectStore('documents').get(documentId);
      transaction.oncomplete = () => resolve(request.result);
      transaction.onerror = transaction.onabort = () => reject(transaction.error ?? new Error('Saved document read failed'));
    });
  }
  async list(documentId: string): Promise<OutboxEntry[]> {
    return new Promise((resolve, reject) => {
      const transaction = this.database.transaction('requests', 'readonly');
      const request = transaction.objectStore('requests').index('document').getAll(documentId);
      transaction.oncomplete = () => resolve((request.result as OutboxEntry[]).map((entry) => ({ ...entry, kind: entry.kind ?? 'native' } as OutboxEntry)).sort((a, b) => a.sequence! - b.sequence!));
      transaction.onerror = () => reject(transaction.error ?? new Error('Draft read failed'));
      transaction.onabort = () => reject(transaction.error ?? new Error('Draft read was interrupted'));
    });
  }
  /** Claim before sending. Only never-sent, causal text edits can combine.
   * The read and replacement are atomic across tabs, including browsers
   * without Web Locks. A lost response always retries the same envelope. */
  claimNext(documentId: string, throughSequence = Infinity): Promise<OutboxEntry | undefined> {
    return new Promise((resolve, reject) => {
      const transaction = this.database.transaction('requests', 'readwrite', { durability: 'strict' });
      const store = transaction.objectStore('requests'); const read = store.index('document').getAll(documentId);
      let claimed: OutboxEntry | undefined;
      transaction.oncomplete = () => resolve(claimed);
      transaction.onerror = transaction.onabort = () => reject(transaction.error ?? new Error('Draft claim failed'));
      read.onsuccess = () => {
        try {
          const pending = (read.result as OutboxEntry[]).map((entry) => ({ ...entry, kind: entry.kind ?? 'native' } as OutboxEntry))
            .filter((entry) => entry.state === 'pending' && entry.sequence! <= throughSequence).sort((a, b) => a.sequence! - b.sequence!);
          const first = pending[0]; if (!first) return;
          const group = [first]; const source = textSource(first);
          if (source && first.attempted === false) {
            for (const next of pending.slice(1, 64)) {
              const previous = group.at(-1)!;
              if (next.attempted !== false || next.kind !== 'native' || textSource(next) !== source
                  || JSON.stringify(next.envelope.actor) !== JSON.stringify(first.envelope.actor)
                  || !previous.heads || JSON.stringify(next.envelope.requests[0]?.base) !== JSON.stringify(previous.heads)) break;
              group.push(next);
            }
          }
          claimed = { ...first, attempted: true };
          if (group.length > 1 && first.kind === 'native') {
            const last = group.at(-1)!; const requestId = crypto.randomUUID();
            claimed = { ...first, attempted: true, requestId, draft: last.draft, heads: last.heads,
              envelope: { ...first.envelope, request_id: requestId,
                requests: group.flatMap((entry) => entry.kind === 'native' ? entry.envelope.requests : []) } };
            for (const entry of group.slice(1)) store.delete(entry.sequence!);
          }
          store.put(claimed);
        } catch (error) { transaction.abort(); reject(error); }
      };
    });
  }
  async remove(sequence: number) { await this.write((store) => store.delete(sequence)); }
  async mark(entry: OutboxEntry, state: OutboxEntry['state'], error?: string) {
    await this.write((store) => {
      const current = store.get(entry.sequence!);
      current.onsuccess = () => {
        // A second tab may already have completed this exact delivery.
        if (current.result?.requestId === entry.requestId) store.put({ ...current.result, state, error });
      };
    });
  }
  private write(edit: (store: IDBObjectStore, transaction: IDBTransaction) => void): Promise<void> {
    return new Promise((resolve, reject) => {
      const transaction = this.database.transaction(['requests', 'documents'], 'readwrite', { durability: 'strict' });
      transaction.oncomplete = () => resolve();
      transaction.onerror = () => reject(transaction.error ?? new Error('Draft write failed'));
      transaction.onabort = () => reject(transaction.error ?? new Error('Draft write was interrupted'));
      try { edit(transaction.objectStore('requests'), transaction); }
      catch (error) { transaction.abort(); reject(error); }
    });
  }
}

export class DocumentRequestError extends Error {
  constructor(readonly status: number, message: string) { super(message); }
}

/** Replays exact persisted requests. A lost response is safe to retry. */
export async function drainDocumentOutbox(
  outbox: DocumentOutbox,
  documentId: string,
  send: (entry: OutboxEntry) => Promise<void>,
  changed: () => void | Promise<void> = () => {},
  signal?: AbortSignal,
) {
  // Finite passes let other tabs acquire delivery ownership during continuous typing.
  const throughSequence = (await outbox.list(documentId)).at(-1)?.sequence;
  if (throughSequence === undefined) return;
  for (;;) {
    signal?.throwIfAborted();
    const entry = await outbox.claimNext(documentId, throughSequence);
    if (!entry) break;
    try {
      signal?.throwIfAborted();
      await send(entry);
      await outbox.remove(entry.sequence!);
      await changed();
    } catch (error) {
      if (error instanceof DocumentRequestError && [400, 401, 403, 404, 405, 409, 410, 412, 413, 422].includes(error.status)) {
        await outbox.mark(entry, 'failed', error.message);
        await changed();
      } else throw error;
    }
  }
}


function textSource(entry: OutboxEntry): string | undefined {
  if (entry.kind !== 'native' || !entry.envelope.requests.length) return;
  const sources = new Set<string>();
  for (const request of entry.envelope.requests) {
    if (!request.commands.length) return;
    for (const command of request.commands) {
      // Inspect only the addressed sources. Combining delivery envelopes never
      // changes an edit's mode, proposal ID, request ID, or native base.
      const action = command.op === 'edit' ? command.action : command;
      if (action.op === 'insert_text') sources.add(action.at.source);
      else if (action.op === 'replace_text') {
        sources.add(action.at.source);
        for (const range of action.ranges) sources.add(range.source);
      }
      else if (action.op === 'delete_text' || action.op === 'format') for (const range of action.ranges) sources.add(range.source);
      else return;
    }
  }
  return sources.size === 1 ? [...sources][0] : undefined;
}
