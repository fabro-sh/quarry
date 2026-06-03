import { describe, expect, it } from 'vitest';
import type { Awareness } from 'y-protocols/awareness';

import {
  COLLAB_AWARENESS_FIELD,
  collectFlushAcks,
  collectRecoveryErrors,
  electFlusherClientId,
  updateCollabAwareness,
} from './flusher-lease';

describe('collab flusher lease', () => {
  it('elects the lowest active awareness client and advertises the local lease only for that client', () => {
    const awareness = new FakeAwareness(7);

    expect(updateCollabAwareness(awareness.value, 'browser:local')).toBe(true);
    expect(electFlusherClientId(awareness.value)).toBe(7);
    expect(awareness.localCollabState()).toMatchObject({
      flusherLease: { clientId: 7, sessionId: 'browser:local' },
      sessionId: 'browser:local',
    });

    awareness.setRemoteCollabState(3, { sessionId: 'browser:peer' });

    expect(updateCollabAwareness(awareness.value, 'browser:local')).toBe(false);
    expect(electFlusherClientId(awareness.value)).toBe(3);
    expect(awareness.localCollabState()).toMatchObject({
      flusherLease: null,
      sessionId: 'browser:local',
    });
  });

  it('collects flush acknowledgements from active peers', () => {
    const awareness = new FakeAwareness(10);
    updateCollabAwareness(awareness.value, 'browser:local', {
      etag: '"v2"',
      sessionId: 'browser:local',
      versionId: 'v2',
    });
    awareness.setRemoteCollabState(11, {
      flushAck: { etag: '"v3"', sessionId: 'browser:peer', versionId: 'v3' },
      sessionId: 'browser:peer',
    });

    expect(collectFlushAcks(awareness.value)).toEqual([
      { etag: '"v2"', sessionId: 'browser:local', versionId: 'v2' },
      { etag: '"v3"', sessionId: 'browser:peer', versionId: 'v3' },
    ]);
  });

  it('collects server recovery persistence errors from awareness', () => {
    const awareness = new FakeAwareness(10);
    awareness.states.set(99, {
      quarryServer: {
        recoveryError: {
          documentId: 'doc-1',
          message: 'failed to persist collab recovery state',
        },
      },
    });

    expect(collectRecoveryErrors(awareness.value)).toEqual([
      {
        documentId: 'doc-1',
        message: 'failed to persist collab recovery state',
      },
    ]);
  });
});

class FakeAwareness {
  readonly states = new Map<number, Record<string, unknown>>();
  private localState: Record<string, unknown> = {};

  constructor(readonly clientID: number) {
    this.states.set(clientID, this.localState);
  }

  get value() {
    return this as unknown as Awareness;
  }

  getLocalState() {
    return this.localState;
  }

  setLocalStateField(field: string, value: unknown) {
    this.localState = { ...this.localState, [field]: value };
    this.states.set(this.clientID, this.localState);
  }

  getStates() {
    return this.states;
  }

  localCollabState() {
    return this.localState[COLLAB_AWARENESS_FIELD];
  }

  setRemoteCollabState(clientId: number, state: unknown) {
    this.states.set(clientId, { [COLLAB_AWARENESS_FIELD]: state });
  }
}
