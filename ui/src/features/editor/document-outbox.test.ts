import 'fake-indexeddb/auto';
import { DocumentOutbox, DocumentRequestError, drainDocumentOutbox, type OutboxEntry } from './document-outbox';

function entry(documentId: string, requestId: string): OutboxEntry {
  return { kind: 'native', documentId, requestId, envelope: { request_id: requestId, requests: [], actor: { kind: 'browser', id: 'tab', label: 'Reviewer' } }, draft: Uint8Array.of(1, 2, 3), state: 'pending' };
}

test('a lost acknowledgement replays the exact persisted request after restart', async () => {
  const name = crypto.randomUUID(); let box = await DocumentOutbox.open(name);
  const original = entry('doc', 'one'); await box.add(original);
  const receipts = new Map(); let commits = 0;
  const send = async (envelope: unknown) => {
    const key = JSON.stringify(envelope);
    if (!receipts.has(key)) { receipts.set(key, true); commits++; throw new Error('Connection closed after commit'); }
  };
  await expect(drainDocumentOutbox(box, 'doc', send)).rejects.toThrow('Connection closed');
  box.close(); box = await DocumentOutbox.open(name);
  expect((await box.list('doc'))[0].envelope).toEqual(original.envelope);
  await drainDocumentOutbox(box, 'doc', send);
  expect(commits).toBe(1); expect(await box.list('doc')).toEqual([]); box.close();
});

test('semantic rejection retains the draft and allows independent requests to finish', async () => {
  const box = await DocumentOutbox.open(crypto.randomUUID());
  await box.add(entry('doc', 'stale')); await box.add(entry('doc', 'comment'));
  const sent: string[] = [];
  await drainDocumentOutbox(box, 'doc', async (envelope) => {
    sent.push(envelope.requestId);
    if (envelope.requestId === 'stale') throw new DocumentRequestError(412, 'Structure changed');
  });
  expect(sent).toEqual(['stale', 'comment']);
  const failed = (await box.list('doc'))[0];
  expect(failed.state).toBe('failed'); expect(Array.from(failed.draft)).toEqual([1, 2, 3]);
  await box.mark(failed, 'archived');
  await drainDocumentOutbox(box, 'doc', async () => { throw new Error('Archived drafts must not replay'); });
  expect((await box.list('doc'))[0].draft).toEqual(failed.draft); box.close();
});

test('two tabs preserve causal insertion order and scope request IDs to each document', async () => {
  const name = crypto.randomUUID(); const first = await DocumentOutbox.open(name); const second = await DocumentOutbox.open(name);
  await first.add(entry('a', 'one')); await second.add(entry('a', 'two')); await second.add(entry('b', 'one'));
  expect((await first.list('a')).map((v) => v.requestId)).toEqual(['one', 'two']);
  expect((await first.list('b')).map((v) => v.requestId)).toEqual(['one']);
  await expect(second.add(entry('a', 'one'))).rejects.toBeTruthy();
  first.close(); second.close();
});

test('cached native state and the exact request commit together and survive reopening', async () => {
  const name = crypto.randomUUID(); let box = await DocumentOutbox.open(name);
  const original = entry('doc', 'one');
  await box.add(original, { documentId: 'doc', document_clock: 'v1', bytes: original.draft });
  await expect(box.add(original, { documentId: 'doc', document_clock: 'corrupt', bytes: Uint8Array.of(9) })).rejects.toBeTruthy();
  box.close(); box = await DocumentOutbox.open(name);
  const cached = (await box.cached('doc'))!;
  expect(cached.documentId).toBe('doc'); expect(cached.document_clock).toBe('v1');
  expect(Array.from(cached.bytes)).toEqual(Array.from(original.draft));
  expect((await box.list('doc'))[0].envelope).toEqual(original.envelope);
  box.close();
});

test('conflict decisions replay the same body after a lost response', async () => {
  const box = await DocumentOutbox.open(crypto.randomUUID());
  const original: OutboxEntry = { ...entry('doc', 'conflict'), kind: 'conflict', envelope: {
    client_tx_id: 'conflict', base_clock: 'version', actor: { kind: 'browser', id: 'tab', label: 'Reviewer' },
    ops: [{ op: 'conflict.accept_incoming', item_id: 'c' }],
  } };
  await box.add(original);
  const bodies: unknown[] = [];
  await expect(drainDocumentOutbox(box, 'doc', async (entry) => { bodies.push(entry.envelope); throw new Error('Lost response'); })).rejects.toThrow();
  await drainDocumentOutbox(box, 'doc', async (entry) => { bodies.push(entry.envelope); });
  expect(bodies).toEqual([original.envelope, original.envelope]);
  expect(await box.list('doc')).toEqual([]); box.close();
});


function typing(id: string, before: string, after: string, source = 'text', intent = true): OutboxEntry {
  const base = entry('doc', id);
  return { ...base, kind: 'native', attempted: false, heads: [after], draft: new TextEncoder().encode(after),
    envelope: { request_id: id, actor: base.envelope.actor, requests: [{ request_id: id + '_0', base: [before], at: '', commands: [intent
      ? { op: 'edit', mode: { kind: 'direct' }, action: { op: 'insert_text', at: { source, cursor: 'cursor' }, text: id } }
      : { op: 'insert_text', at: { source, cursor: 'cursor' }, text: id }] }] } };
}

test.each([false, true])('rapid causal typing commits one immutable delivery and replays it after a lost response (edit intent: %s)', async (intent) => {
  const name = crypto.randomUUID(); let box = await DocumentOutbox.open(name);
  for (let n = 0; n < 40; n++) await box.add(typing('key' + n, 'head' + n, 'head' + (n + 1), 'text', intent));
  const envelopes: unknown[] = [];
  await expect(drainDocumentOutbox(box, 'doc', async (entry) => { envelopes.push(entry.envelope); throw new Error('Response lost after durable commit'); })).rejects.toThrow('Response lost');
  const rows = await box.list('doc'); expect(rows).toHaveLength(1); expect(rows[0].attempted).toBe(true);
  expect(new TextDecoder().decode(rows[0].draft)).toBe('head40');
  if (rows[0].kind !== 'native') throw new Error('Expected native delivery');
  expect(rows[0].envelope.requests.map((r) => r.request_id)).toEqual(Array.from({ length: 40 }, (_, n) => 'key' + n + '_0'));
  box.close(); box = await DocumentOutbox.open(name);
  await box.add(typing('later', 'head40', 'head41'));
  await drainDocumentOutbox(box, 'doc', async (entry) => { envelopes.push(entry.envelope); });
  expect(envelopes[1]).toEqual(envelopes[0]); expect(envelopes).toHaveLength(3);
  expect(await box.list('doc')).toEqual([]); box.close();
});

test('one delivery preserves separate suggestion identities and explicit continuations', async () => {
  const box = await DocumentOutbox.open(crypto.randomUUID());
  try {
    const original = [typing('new', 'base', 'one'), typing('continue', 'one', 'two'), typing('separate', 'two', 'three')];
    for (const [index, item] of original.entries()) {
      if (item.kind !== 'native') throw new Error('Expected native edit');
      item.envelope.requests[0].commands = [{ op: 'edit',
        mode: { kind: index === 1 ? 'continue' : 'suggest', id: index === 2 ? 'separate' : 'first', author: 'Reviewer' },
        action: { op: 'replace_text', at: { source: 'text', cursor: 'cursor' }, ranges: [{ source: 'text', start: 'start', end: 'end' }], text: '' } }];
      await box.add(item);
    }
    const claimed = await box.claimNext('doc');
    if (claimed?.kind !== 'native') throw new Error('Expected native delivery');
    expect(claimed.envelope.requests).toEqual(original.flatMap((item) => item.kind === 'native' ? item.envelope.requests : []));
    expect(await box.list('doc')).toHaveLength(1);
    expect((await box.claimNext('doc'))?.envelope).toEqual(claimed.envelope);
  } finally { box.close(); }
});

test('concurrent tab claims cannot replace an attempted delivery or combine unrelated sources and branches', async () => {
  const name = crypto.randomUUID(); const a = await DocumentOutbox.open(name); const b = await DocumentOutbox.open(name);
  await a.add(typing('one', 'base', 'one')); await a.add(typing('two', 'one', 'two'));
  const [first, second] = await Promise.all([a.claimNext('doc'), b.claimNext('doc')]);
  expect(first?.envelope).toEqual(second?.envelope); expect((await a.list('doc')).length).toBe(1);
  await a.remove(first!.sequence!);
  await a.add(typing('branchA', 'base', 'a')); await a.add(typing('branchB', 'base', 'b'));
  await a.add(typing('otherSource', 'b', 'c', 'other'));
  const requests: string[] = [];
  await drainDocumentOutbox(a, 'doc', async (entry) => { requests.push(entry.requestId); });
  expect(requests).toEqual(['branchA', 'branchB', 'otherSource']);
  a.close(); b.close();
});

test('a drain yields to remote refresh before newly queued work and late failures cannot restore committed rows', async () => {
  const box = await DocumentOutbox.open(crypto.randomUUID());
  await box.add(typing('one', 'base', 'one'));
  let sent: OutboxEntry | undefined;
  await drainDocumentOutbox(box, 'doc', async (entry) => {
    sent = entry;
    await box.add(typing('two', 'one', 'two'));
  });
  expect((await box.list('doc')).map((entry) => entry.requestId)).toEqual(['two']);
  await box.mark(sent!, 'failed', 'A late response from another tab');
  expect((await box.list('doc')).map((entry) => entry.requestId)).toEqual(['two']);
  await drainDocumentOutbox(box, 'doc', async () => {});
  expect(await box.list('doc')).toEqual([]); box.close();
});
