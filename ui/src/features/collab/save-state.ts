import * as Y from 'yjs';

// The Phase 5 save-state model: two inputs, three states.
//
// - Websocket connection + sync state: disconnected or unsynced means the
//   editor is read-only and reseeding — `Reconnecting (read-only)`.
// - Checkpoint coverage: the server broadcasts a checkpoint-ack frame (the
//   committed doc state as a Yjs snapshot) after every durable commit and on
//   join (see `MSG_QUARRY_CHECKPOINT` in rust-ws-provider.ts and the
//   `session.rs` module docs). When the acked snapshot equals the local doc,
//   everything on screen is canonical — `Saved`; anything beyond the ack is
//   still owed a commit — `Saving…`.
//
// Coverage is equality, not containment: ack frames are broadcast on the
// same ordered socket AFTER the updates they cover, so by the time an ack
// arrives the local doc is always a superset of the acked state — equality
// IS containment at receipt time. Comparing snapshots (state vector plus
// delete set) rather than state vectors alone keeps pure deletions honest:
// a deletion advances no clock but must still flip the state to Saving…
// until a checkpoint covers it.

export type CollabSaveState = 'saved' | 'saving' | 'reconnecting';

/** Whether the last acked checkpoint covers everything in the local doc. */
export function checkpointCoversDoc(ackedSnapshot: Uint8Array | null, doc: Y.Doc): boolean {
  if (!ackedSnapshot) return false;
  try {
    return Y.equalSnapshots(Y.decodeSnapshot(ackedSnapshot), Y.snapshot(doc));
  } catch {
    return false;
  }
}

export function collabSaveState(input: {
  connected: boolean;
  synced: boolean;
  covered: boolean;
}): CollabSaveState {
  if (!input.connected || !input.synced) return 'reconnecting';
  return input.covered ? 'saved' : 'saving';
}

export function saveStateLabel(state: CollabSaveState): string {
  const label: Record<CollabSaveState, string> = {
    saved: 'Saved',
    saving: 'Saving…',
    reconnecting: 'Reconnecting (read-only)',
  };
  return label[state];
}
