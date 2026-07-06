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

// 'refused' is set directly (never computed by collabSaveState): the server
// closed the socket with the session-refused close code, meaning this
// document cannot host a live session at all — retrying is pointless (see
// COLLAB_SESSION_REFUSED_CLOSE_CODE in rust-ws-provider.ts).
export type CollabSaveState = 'saved' | 'saving' | 'save_failed' | 'reconnecting' | 'refused';

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
  saveFailed: boolean;
}): CollabSaveState {
  if (!input.connected || !input.synced) return 'reconnecting';
  if (input.covered) return 'saved';
  // A failed checkpoint retries on the next edit; the flag clears on the
  // next successful ack (see rust-ws-provider.ts). Until then, "Saving…"
  // would misread as benign progress while nothing is being persisted.
  return input.saveFailed ? 'save_failed' : 'saving';
}

export function saveStateLabel(state: CollabSaveState): string {
  const label: Record<CollabSaveState, string> = {
    saved: 'Saved',
    saving: 'Saving…',
    save_failed: 'Save failed',
    reconnecting: 'Reconnecting (read-only)',
    refused: 'Live editing unavailable',
  };
  return label[state];
}
