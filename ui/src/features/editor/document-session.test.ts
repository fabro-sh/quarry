import 'fake-indexeddb/auto';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { initSync } from '../../generated/document/quarry_document';
import { DocumentModel, type DocumentBatch } from './document-model';
import { DocumentSession, type SessionStatus } from './document-session';
import { DocumentOutbox } from './document-outbox';
import { waitFor } from '@testing-library/react';

beforeAll(() => { initSync({ module: readFileSync(resolve(process.cwd(), 'src/generated/document/quarry_document_bg.wasm')) }); });
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });

function authority() {
  const model = DocumentModel.create(crypto.randomUUID());
  model.edit((draft) => draft.apply([{ op: 'insert_block', block: { id: 'p', kind: 'p', parent: null, position: 0, attrs: {}, text: 'TARGET' } }]));
  let offline = false; let status = 200; let commits = 0;
  const receipts = new Set<string>(); const urls: string[] = [];
  vi.stubGlobal('WebSocket', class { close() {} });
  vi.stubGlobal('fetch', vi.fn(async (url: string, options?: RequestInit) => {
    urls.push(url);
    if (offline) throw new TypeError('Network unavailable');
    if (status !== 200) return Response.json({ message: 'Unavailable' }, { status });
    if (url.includes('/document-state')) {
      const since = new URL(url, 'http://test').searchParams.get('since');
      const base = since ? since.split(',') : null;
      return Response.json({ format: 'automerge', base, document: { document_id: model.view().document_id, heads: model.heads() }, document_clock: String(commits), bytes: Array.from(base ? model.changesSince(base) : model.save()) });
    }
    if (url.includes('/document-commands')) {
      const batch = JSON.parse(String(options?.body)) as DocumentBatch;
      if (!receipts.has(batch.request_id)) { model.applyBatch(batch); receipts.add(batch.request_id); commits++; }
      return Response.json({ request_id: batch.request_id });
    }
    throw new Error('Unexpected request');
  }));
  return { model, urls, offline(value: boolean) { offline = value; }, status(value: number) { status = value; }, commits: () => commits };
}

test('an offline draft survives closing, cold reopening, and exact replay on reconnect', async () => {
  const server = authority(); const id = server.model.view().document_id;
  let session: DocumentSession | undefined;
  try {
    session = await DocumentSession.open('/document', id, 'Reviewer', 'tab');
    server.offline(true);
    let state: SessionStatus | undefined;
    session.onStatus = (next) => { state = next; };
    const batch = session.model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point('p', 0), text: 'Offline ' }]));
    session.enqueue(batch);
    await waitFor(() => expect(state?.state).toBe('save_failed'));
    session.close(); session.model.dispose();
    session = await DocumentSession.open('/document', id, 'Reviewer', 'tab');
    expect(session.model.view().blocks[0].text).toBe('Offline TARGET');
    session.onStatus = (next) => { state = next; };
    server.offline(false); session.start();
    await waitFor(() => expect(state?.state).toBe('saved'));
    expect(server.model.view().blocks[0].text).toBe('Offline TARGET');
    expect(server.commits()).toBe(1);
  } finally { session?.close(); session?.model.dispose(); server.model.dispose(); }
});

test('a rejected capability never falls back to an editable cached document', async () => {
  const server = authority(); const id = server.model.view().document_id;
  const session = await DocumentSession.open('/document', id, 'Reviewer', 'tab');
  session.close(); session.model.dispose();
  try {
    for (const status of [401, 403, 404, 410]) {
      server.status(status);
      await expect(DocumentSession.open('/document', id, 'Reviewer', 'tab')).rejects.toMatchObject({ status });
    }
  } finally { server.model.dispose(); }
});

test('library transport uses native identity and sends its invitation on every request', async () => {
  const server = authority(); const id = server.model.view().document_id;
  const session = await DocumentSession.open('/v1/libraries/notes/documents/old.md', id, 'Reviewer', 'tab', 'invitation');
  try {
    let state: SessionStatus | undefined;
    session.onStatus = (next) => { state = next; };
    session.enqueue(session.model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point('p', 0), text: 'Edit ' }])));
    await waitFor(() => expect(state?.state).toBe('saved'));
    expect(server.urls.every((url) => url.startsWith(`/v1/libraries/notes/documents-by-id/${id}/`))).toBe(true);
    expect(server.urls.every((url) => new URL(url, 'http://test').searchParams.get('token') === 'invitation')).toBe(true);
  } finally { session.close(); session.model.dispose(); server.model.dispose(); }
});

test('remote state waits for IME composition and merges with text entered against the displayed version', async () => {
  const server = authority(); const id = server.model.view().document_id;
  const session = await DocumentSession.open('/document', id, 'Reviewer', 'tab');
  try {
    session.setComposing(true);
    server.model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point('p', 0), text: 'Outside ' }]));
    let status: SessionStatus | undefined;
    session.onStatus = (value) => { status = value; };
    session.start();
    await waitFor(() => expect(status?.state).toBe('saved'));
    expect(session.model.view().blocks[0].text).toBe('TARGET');
    status = undefined;
    server.model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point('p', 0), text: 'Second ' }]));
    window.dispatchEvent(new Event('online'));
    await waitFor(() => expect(status?.state).toBe('saved'));
    expect(session.model.view().blocks[0].text).toBe('TARGET');
    status = undefined;
    session.enqueue(session.model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point('p', 3), text: '你好' }])));
    await waitFor(() => expect(status?.state).toBe('saved'));
    expect(session.model.view().blocks[0].text).toBe('TAR你好GET');
    session.setComposing(false);
    expect(session.model.view().blocks[0].text).toBe('Second Outside TAR你好GET');
    expect(session.model.view()).toEqual(server.model.view());
  } finally { session.close(); session.model.dispose(); server.model.dispose(); }
});

test('queued drafts keep a valid receive base when remote changes arrive before storage finishes', async () => {
  const server = authority(); const id = server.model.view().document_id;
  let session = await DocumentSession.open('/document', id, 'Reviewer', 'tab');
  const outbox = await DocumentOutbox.open();
  let releaseRead!: () => void; let readStarted!: () => void; let releaseWrite!: () => void;
  const reading = new Promise<void>((resolve) => { readStarted = resolve; });
  const readGate = new Promise<void>((resolve) => { releaseRead = resolve; });
  const writeGate = new Promise<void>((resolve) => { releaseWrite = resolve; });
  try {
    const fetch = globalThis.fetch; let firstRead = true;
    vi.stubGlobal('fetch', vi.fn(async (...args: Parameters<typeof fetch>) => {
      const response = await fetch(...args);
      if (String(args[0]).includes('/document-state') && firstRead) {
        firstRead = false; readStarted(); await readGate;
      }
      return response;
    }));
    const add = DocumentOutbox.prototype.add;
    const writing = vi.spyOn(DocumentOutbox.prototype, 'add').mockImplementationOnce(async function (this: DocumentOutbox, entry, cached) {
      await writeGate; return add.call(this, entry, cached);
    });
    server.model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point('p', 0), text: 'Remote ' }]));
    let status: SessionStatus | undefined;
    session.onStatus = (value) => { status = value; };
    session.start(); await reading;
    session.enqueue(session.model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point('p', 3), text: 'A' }])));
    session.enqueue(session.model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point('p', 4), text: 'B' }])));
    await waitFor(() => expect(writing).toHaveBeenCalledTimes(1));
    releaseRead();
    await waitFor(() => expect(session.model.view().blocks[0].text).toBe('Remote TARABGET'));
    server.offline(true); releaseWrite();
    await waitFor(() => expect(status?.state).toBe('save_failed'));
    await session.close();
    const cached = (await outbox.cached(id))!; const snapshot = new DocumentModel(Uint8Array.from(cached.bytes));
    try { expect(snapshot.containsHistory(cached.heads!)).toBe(true); }
    finally { snapshot.dispose(); }

    const previous = session;
    session = await DocumentSession.open('/document', id, 'Reviewer', 'tab');
    previous.model.dispose();
    status = undefined; session.onStatus = (value) => { status = value; };
    server.offline(false); session.start();
    await waitFor(() => expect(status?.state).toBe('saved'));
    expect(session.model.view().blocks[0].text).toBe('Remote TARABGET');
    expect(session.model.view()).toEqual(server.model.view());
  } finally { releaseRead(); releaseWrite(); await session.close(); session.model.dispose(); outbox.close(); server.model.dispose(); }
});

test('Saved never describes an older drain when a new edit finishes storage during a state refresh', async () => {
  const server = authority();
  const id = server.model.view().document_id;
  const session = await DocumentSession.open('/document', id, 'Reviewer', 'tab');
  const outbox = await DocumentOutbox.open();
  let release!: () => void, entered!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const caching = new Promise<void>((resolve) => { entered = resolve; });
  const statuses: Array<{ state: string; commits: number }> = [];
  const cache = DocumentOutbox.prototype.cache;
  vi.spyOn(DocumentOutbox.prototype, 'cache').mockImplementationOnce(async function (this: DocumentOutbox, ...args) {
    entered(); await gate; return cache.apply(this, args);
  });
  try {
    session.onStatus = (value) => statuses.push({ state: value.state, commits: server.commits() });
    session.start(); await caching;
    statuses.length = 0; // Opening an unchanged document is already Saved.
    session.enqueue(session.model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point('p', 0), text: 'New ' }])));
    await waitFor(async () => expect((await outbox.list(id)).filter((entry) => entry.state === 'pending')).toHaveLength(1));
    await waitFor(() => expect(statuses.at(-1)).toEqual({ state: 'saved', commits: 1 }));
    expect(statuses.filter((value) => value.state === 'saved').every((value) => value.commits === 1)).toBe(true);
    expect(server.model.view().blocks[0].text).toBe('New TARGET');
  } finally { release(); await session.close(); session.model.dispose(); outbox.close(); server.model.dispose(); }
});

for (const resumeImmediately of [false, true]) test(`page navigation cancels reads and resumes durable edits (immediate restore: ${resumeImmediately})`, async () => {
  const server = authority(); const id = server.model.view().document_id;
  const session = await DocumentSession.open('/document', id, 'Reviewer', 'tab');
  const outbox = await DocumentOutbox.open();
  const statuses: SessionStatus[] = [];
  let signal: AbortSignal | undefined;
  const fetch = globalThis.fetch;
  vi.stubGlobal('fetch', vi.fn((...args: Parameters<typeof fetch>) => {
    if (String(args[0]).includes('/document-state') && !signal) {
      signal = args[1]?.signal ?? undefined;
      return new Promise<Response>((_resolve, reject) => signal?.addEventListener('abort', () => reject(signal!.reason), { once: true }));
    }
    return fetch(...args);
  }));
  try {
    session.onStatus = (value) => statuses.push(value);
    session.start();
    await waitFor(() => expect(signal).toBeDefined());
    window.dispatchEvent(new Event('pagehide'));
    expect(signal!.aborted).toBe(true);
    session.enqueue(session.model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point('p', 0), text: 'Local ' }])));
    server.model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point('p', 6), text: ' remote' }]));
    if (!resumeImmediately) {
      await waitFor(async () => expect(await outbox.list(id)).toHaveLength(1));
      window.dispatchEvent(new Event('online'));
      expect(server.commits()).toBe(0);
      expect(session.model.view().blocks[0].text).toBe('Local TARGET');
    }
    window.dispatchEvent(new Event('pageshow'));
    await waitFor(() => expect(statuses.at(-1)?.state).toBe('saved'));
    expect(statuses.some((value) => value.state === 'save_failed')).toBe(false);
    expect(server.commits()).toBe(1);
    expect(session.model.view().blocks[0].text).toBe('Local TARGET remote');
    expect(session.model.view()).toEqual(server.model.view());
    expect(await outbox.list(id)).toHaveLength(0);
  } finally { await session.close(); session.model.dispose(); outbox.close(); server.model.dispose(); }
});

test('closing a session cancels an unfinished state read without waiting for the network', async () => {
  const server = authority(); const id = server.model.view().document_id;
  const session = await DocumentSession.open('/document', id, 'Reviewer', 'tab');
  let signal: AbortSignal | undefined;
  const fetch = globalThis.fetch;
  vi.stubGlobal('fetch', vi.fn((...args: Parameters<typeof fetch>) => {
    if (String(args[0]).includes('/document-state')) {
      signal = args[1]?.signal ?? undefined;
      return new Promise<Response>((_resolve, reject) => signal?.addEventListener('abort', () => reject(signal!.reason), { once: true }));
    }
    return fetch(...args);
  }));
  try {
    session.start();
    await waitFor(() => expect(signal).toBeDefined());
    await session.close();
    expect(signal!.aborted).toBe(true);
    const reads = server.urls.length;
    window.dispatchEvent(new Event('pageshow'));
    window.dispatchEvent(new Event('online'));
    expect(server.urls).toHaveLength(reads);
  } finally { await session.close(); session.model.dispose(); server.model.dispose(); }
});

test('page navigation cancels an attempt and retries the exact durable command without a second commit', async () => {
  const server = authority(); const id = server.model.view().document_id;
  const session = await DocumentSession.open('/document', id, 'Reviewer', 'tab');
  const statuses: SessionStatus[] = [];
  const fetch = globalThis.fetch;
  let release!: () => void; let sent = false; let cancelled = false; let posts = 0;
  const bodies: string[] = [];
  const gate = new Promise<void>((resolve) => { release = resolve; });
  vi.stubGlobal('fetch', vi.fn(async (...args: Parameters<typeof fetch>) => {
    const response = await fetch(...args);
    if (String(args[0]).includes('/document-commands')) {
      posts++; sent = true; bodies.push(String(args[1]?.body));
      args[1]?.signal?.addEventListener('abort', () => { cancelled = true; });
      await gate;
    }
    return response;
  }));
  try {
    session.onStatus = (value) => statuses.push(value);
    session.start();
    session.enqueue(session.model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point('p', 0), text: 'Local ' }])));
    await waitFor(() => expect(sent).toBe(true));
    window.dispatchEvent(new Event('pagehide'));
    expect(cancelled).toBe(true);
    window.dispatchEvent(new Event('pageshow'));
    release();
    await waitFor(() => expect(statuses.at(-1)?.state).toBe('saved'));
    expect(statuses.some((value) => value.state === 'save_failed')).toBe(false);
    expect(posts).toBe(2); expect(bodies[1]).toBe(bodies[0]); expect(server.commits()).toBe(1);
    expect(session.model.view()).toEqual(server.model.view());
    expect(session.model.view().blocks[0].text).toBe('Local TARGET');
  } finally { release(); await session.close(); session.model.dispose(); server.model.dispose(); }
});

const fastTiming = { requestTimeoutMs: 300, retryMs: 30, pollMs: 100, deliveryDelayMs: 10 };

test('a held refresh cannot block delivery or turn an acknowledged save into a failure', async () => {
  const server = authority(), id = server.model.view().document_id;
  const session = await DocumentSession.open('/document', id, 'Writer', 'tab');
  const outbox = await DocumentOutbox.open(), fetch = globalThis.fetch;
  let failRead!: () => void, reading = false;
  const heldRead = new Promise<Response>((resolve) => { failRead = () => resolve(Response.json({ message: 'Refresh unavailable' }, { status: 503 })); });
  vi.stubGlobal('fetch', vi.fn((...args: Parameters<typeof fetch>) => {
    if (String(args[0]).includes('/document-state')) { reading = true; return heldRead; }
    return fetch(...args);
  }));
  let status: SessionStatus | undefined; session.onStatus = (value) => { status = value; };
  try {
    session.start(); await waitFor(() => expect(reading).toBe(true));
    session.enqueue(session.model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point('p', 0), text: 'Local ' }])));
    await waitFor(() => expect(server.commits()).toBe(1));
    await waitFor(() => expect(status?.state).toBe('saved'));
    expect(await outbox.list(id)).toEqual([]); expect(status?.sync).toBe('refreshing');
    failRead(); await waitFor(() => expect(status?.sync).toBe('reconnecting'));
    expect(status?.state).toBe('saved'); expect(status?.blocked).toBe(false);
    expect(server.model.view().blocks[0].text).toBe('Local TARGET');
  } finally { failRead(); await session.close(); session.model.dispose(); outbox.close(); server.model.dispose(); }
});

test('buffered editor work and delayed local persistence prevent Saved until acknowledgement', async () => {
  const server = authority(), id = server.model.view().document_id;
  const session = await DocumentSession.open('/document', id, 'Writer', 'tab', undefined, fastTiming);
  let status: SessionStatus | undefined; session.onStatus = (value) => { status = value; };
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; }), add = DocumentOutbox.prototype.add;
  vi.spyOn(DocumentOutbox.prototype, 'add').mockImplementationOnce(async function (this: DocumentOutbox, entry, cache) { await gate; return add.call(this, entry, cache); });
  try {
    session.setBufferedEdit(true); session.start();
    await waitFor(() => expect(status?.sync).toBe('current'));
    expect(status?.state).toBe('saving'); expect(server.commits()).toBe(0);
    session.enqueue(session.model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point('p', 0), text: 'Local ' }])));
    session.setBufferedEdit(false);
    expect(status?.state).toBe('saving'); expect(server.commits()).toBe(0);
    release(); await waitFor(() => expect(status?.state).toBe('saved'));
    expect(server.commits()).toBe(1);
  } finally { release(); await session.close(); session.model.dispose(); server.model.dispose(); }
});

test('periodic refresh recovers silent notification loss without affecting save state', async () => {
  const server = authority(), id = server.model.view().document_id;
  const session = await DocumentSession.open('/document', id, 'Writer', 'tab', undefined, fastTiming);
  const statuses: SessionStatus[] = []; session.onStatus = (value) => statuses.push(value);
  try {
    session.start(); await waitFor(() => expect(statuses.at(-1)?.sync).toBe('current'));
    server.model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point('p', 0), text: 'Remote ' }]));
    // No event, online signal, or local edit wakes this session.
    await waitFor(() => expect(session.model.view()).toEqual(server.model.view()));
    expect(statuses.every((status) => status.state === 'saved')).toBe(true);
  } finally { await session.close(); session.model.dispose(); server.model.dispose(); }
});

test('a timed-out acknowledgement retries its original envelope before newly queued edits', async () => {
  const server = authority(), id = server.model.view().document_id;
  const session = await DocumentSession.open('/document', id, 'Writer', 'tab', undefined, fastTiming);
  const fetch = globalThis.fetch, bodies: string[] = [];
  let sent!: () => void; const committed = new Promise<void>((resolve) => { sent = resolve; });
  vi.stubGlobal('fetch', vi.fn(async (...args: Parameters<typeof fetch>) => {
    const response = await fetch(...args);
    if (String(args[0]).includes('/document-commands')) {
      bodies.push(String(args[1]?.body));
      if (bodies.length === 1) { sent(); return new Promise<Response>(() => {}); }
    }
    return response;
  }));
  let status: SessionStatus | undefined; session.onStatus = (value) => { status = value; };
  try {
    session.start();
    session.enqueue(session.model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point('p', 0), text: 'First ' }])));
    await committed;
    session.enqueue(session.model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point('p', 0), text: 'Second ' }])));
    await waitFor(() => expect(status?.state).toBe('saved'), { timeout: 3000 });
    expect(bodies).toHaveLength(3); expect(bodies[1]).toBe(bodies[0]); expect(bodies[2]).not.toBe(bodies[0]);
    expect(server.commits()).toBe(2); expect(server.model.view().blocks[0].text).toBe('Second First TARGET');
  } finally { await session.close(); session.model.dispose(); server.model.dispose(); }
});

test('closing the delivering tab releases an uncertain attempt for another session to replay', async () => {
  const server = authority(), id = server.model.view().document_id;
  const first = await DocumentSession.open('/document', id, 'Writer', 'one', undefined, fastTiming);
  const second = await DocumentSession.open('/document', id, 'Writer', 'two', undefined, fastTiming);
  const fetch = globalThis.fetch, bodies: string[] = [];
  vi.stubGlobal('fetch', vi.fn(async (...args: Parameters<typeof fetch>) => {
    const response = await fetch(...args);
    if (String(args[0]).includes('/document-commands')) {
      bodies.push(String(args[1]?.body));
      if (bodies.length === 1) return new Promise<Response>(() => {});
    }
    return response;
  }));
  let status: SessionStatus | undefined; second.onStatus = (value) => { status = value; };
  try {
    first.start();
    first.enqueue(first.model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point('p', 0), text: 'Persisted ' }])));
    await waitFor(() => expect(bodies).toHaveLength(1)); await first.close();
    second.start(); await waitFor(() => expect(bodies).toHaveLength(2));
    await waitFor(() => expect(status?.state).toBe('saved'));
    expect(bodies[1]).toBe(bodies[0]); expect(server.commits()).toBe(1);
    await waitFor(() => expect(second.model.view()).toEqual(server.model.view()));
  } finally { await first.close(); await second.close(); first.model.dispose(); second.model.dispose(); server.model.dispose(); }
});

test('a stale response from before an explicit reset cannot restore old metadata or state', async () => {
  const server = authority(), id = server.model.view().document_id;
  const session = await DocumentSession.open('/document', id, 'Writer', 'tab', undefined, fastTiming);
  const fetch = globalThis.fetch;
  let release!: () => void, held = false;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  vi.stubGlobal('fetch', vi.fn(async (...args: Parameters<typeof fetch>) => {
    const response = await fetch(...args);
    if (String(args[0]).includes('/document-state')) {
      const payload = await response.json();
      if (!held) { held = true; await gate; return Response.json({ ...payload, metadata: { label: 'old' } }); }
      return Response.json({ ...payload, metadata: { label: 'new' } });
    }
    return response;
  }));
  try {
    session.start(); await waitFor(() => expect(held).toBe(true));
    server.model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point('p', 0), text: 'Remote ' }]));
    await session.useSavedVersion(); release();
    await waitFor(() => expect(session.metadata).toEqual({ label: 'new' }));
    expect(session.model.view()).toEqual(server.model.view());
  } finally { release(); await session.close(); session.model.dispose(); server.model.dispose(); }
});

test('seeing the committed history cannot acknowledge a command with the wrong receipt identity', async () => {
  const server = authority(), id = server.model.view().document_id;
  const session = await DocumentSession.open('/document', id, 'Writer', 'tab', undefined, fastTiming);
  const outbox = await DocumentOutbox.open(), fetch = globalThis.fetch, bodies: string[] = [];
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  vi.stubGlobal('fetch', vi.fn(async (...args: Parameters<typeof fetch>) => {
    const response = await fetch(...args);
    if (String(args[0]).includes('/document-commands')) {
      bodies.push(String(args[1]?.body));
      if (bodies.length === 1) return Response.json({ request_id: 'another-request' });
      await gate;
    }
    return response;
  }));
  let status: SessionStatus | undefined; session.onStatus = (value) => { status = value; };
  try {
    session.start();
    session.enqueue(session.model.edit((draft) => draft.apply([{ op: 'insert_text', at: draft.model.point('p', 0), text: 'Kept ' }])));
    await waitFor(() => expect(bodies).toHaveLength(2));
    expect(status?.state).not.toBe('saved');
    expect((await outbox.list(id)).filter((entry) => entry.state === 'pending')).toHaveLength(1);
    expect(server.commits()).toBe(1);
    expect(bodies[1]).toBe(bodies[0]);
    release();
    await waitFor(() => expect(status?.state).toBe('saved'));
    expect(await outbox.list(id)).toHaveLength(0);
    expect(server.commits()).toBe(1);
  } finally { release(); await session.close(); outbox.close(); session.model.dispose(); server.model.dispose(); }
});
