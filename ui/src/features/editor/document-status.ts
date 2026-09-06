/** A save is complete only when the server acknowledges the durable request. */
export type DocumentSaveState = 'saved' | 'saving' | 'save_failed' | 'reconnecting' | 'refused';
export function saveStateLabel(state: DocumentSaveState): string {
  return ({ saved: 'Saved', saving: 'Saving…', save_failed: 'Save failed', reconnecting: 'Reconnecting', refused: 'Editing unavailable' })[state];
}
