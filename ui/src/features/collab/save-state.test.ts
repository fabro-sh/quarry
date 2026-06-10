import { describe, expect, it } from 'vitest';
import * as Y from 'yjs';

import { checkpointCoversDoc, collabSaveState, saveStateLabel } from './save-state';

function snapshotOf(doc: Y.Doc): Uint8Array {
  return Y.encodeSnapshot(Y.snapshot(doc));
}

describe('checkpointCoversDoc', () => {
  it('is false until any ack arrives, even for an empty doc', () => {
    expect(checkpointCoversDoc(null, new Y.Doc())).toBe(false);
  });

  it('covers a doc whose state equals the acked snapshot', () => {
    const doc = new Y.Doc();
    doc.getText('content').insert(0, 'hello');
    expect(checkpointCoversDoc(snapshotOf(doc), doc)).toBe(true);
  });

  it('stops covering after a local insert beyond the ack', () => {
    const doc = new Y.Doc();
    doc.getText('content').insert(0, 'hello');
    const ack = snapshotOf(doc);
    doc.getText('content').insert(5, '!');
    expect(checkpointCoversDoc(ack, doc)).toBe(false);
  });

  it('stops covering after a pure deletion (no clock advances)', () => {
    const doc = new Y.Doc();
    doc.getText('content').insert(0, 'hello');
    const ack = snapshotOf(doc);
    doc.getText('content').delete(0, 2);
    expect(checkpointCoversDoc(ack, doc)).toBe(false);
  });

  it('covers a replica that received the acked state over the wire', () => {
    const server = new Y.Doc();
    server.getText('content').insert(0, 'seeded');
    const client = new Y.Doc();
    Y.applyUpdate(client, Y.encodeStateAsUpdate(server));
    expect(checkpointCoversDoc(snapshotOf(server), client)).toBe(true);
  });

  it('covers across replicas after deletions sync both ways', () => {
    const server = new Y.Doc();
    server.getText('content').insert(0, 'shared text');
    const client = new Y.Doc();
    Y.applyUpdate(client, Y.encodeStateAsUpdate(server));
    client.getText('content').delete(0, 7);
    Y.applyUpdate(server, Y.encodeStateAsUpdate(client));
    expect(checkpointCoversDoc(snapshotOf(server), client)).toBe(true);
  });

  it('rejects garbage ack payloads instead of throwing', () => {
    expect(checkpointCoversDoc(new Uint8Array([7, 7, 7]), new Y.Doc())).toBe(false);
  });
});

describe('collabSaveState', () => {
  it('reports reconnecting whenever the socket is down or unsynced', () => {
    expect(collabSaveState({ connected: false, synced: false, covered: true })).toBe(
      'reconnecting'
    );
    expect(collabSaveState({ connected: true, synced: false, covered: true })).toBe(
      'reconnecting'
    );
  });

  it('reports saving until the checkpoint covers the doc, then saved', () => {
    expect(collabSaveState({ connected: true, synced: true, covered: false })).toBe('saving');
    expect(collabSaveState({ connected: true, synced: true, covered: true })).toBe('saved');
  });
});

describe('saveStateLabel', () => {
  it('maps the three states to their UI labels', () => {
    expect(saveStateLabel('saved')).toBe('Saved');
    expect(saveStateLabel('saving')).toBe('Saving…');
    expect(saveStateLabel('reconnecting')).toBe('Reconnecting (read-only)');
  });
});
