import type { Awareness } from 'y-protocols/awareness';

export const COLLAB_AWARENESS_FIELD = 'quarryCollab';

export interface CollabFlushAck {
  etag: string;
  sessionId: string;
  versionId: string;
}

export interface CollabRecoveryError {
  documentId?: string;
  message: string;
}

export interface CollabAwarenessState {
  flushAck?: CollabFlushAck | null;
  flusherLease?: {
    clientId: number;
    sessionId: string;
  } | null;
  sessionId: string;
}

export function updateCollabAwareness(
  awareness: Awareness,
  sessionId: string,
  flushAck?: CollabFlushAck | null
) {
  const leader = electFlusherClientId(awareness);
  const state: CollabAwarenessState = {
    flushAck: flushAck ?? null,
    flusherLease:
      leader === awareness.clientID
        ? {
            clientId: awareness.clientID,
            sessionId,
          }
        : null,
    sessionId,
  };
  setLocalCollabState(awareness, state);
  return leader === awareness.clientID;
}

export function clearCollabAwareness(awareness: Awareness) {
  setLocalCollabState(awareness, null);
}

export function collectFlushAcks(awareness: Awareness): CollabFlushAck[] {
  const acks: CollabFlushAck[] = [];
  for (const state of awareness.getStates().values()) {
    const ack = collabStateOf(state)?.flushAck;
    if (ack?.etag && ack.versionId && ack.sessionId) acks.push(ack);
  }
  return acks;
}

export function collectRecoveryErrors(awareness: Awareness): CollabRecoveryError[] {
  const errors: CollabRecoveryError[] = [];
  for (const state of awareness.getStates().values()) {
    const error = serverStateOf(state)?.recoveryError;
    if (typeof error?.message === 'string') {
      errors.push({
        documentId: typeof error.documentId === 'string' ? error.documentId : undefined,
        message: error.message,
      });
    }
  }
  return errors;
}

export function electFlusherClientId(awareness: Awareness) {
  const candidates = Array.from(awareness.getStates().entries())
    .filter(([, state]) => Boolean(collabStateOf(state)?.sessionId))
    .map(([clientId]) => clientId);
  if (!candidates.includes(awareness.clientID)) candidates.push(awareness.clientID);
  return Math.min(...candidates);
}

function setLocalCollabState(awareness: Awareness, state: CollabAwarenessState | null) {
  const current = collabStateOf(awareness.getLocalState());
  if (JSON.stringify(current ?? null) === JSON.stringify(state)) return;
  awareness.setLocalStateField(COLLAB_AWARENESS_FIELD, state);
}

function collabStateOf(state: Record<string, unknown> | null): CollabAwarenessState | null {
  const value = state?.[COLLAB_AWARENESS_FIELD];
  if (!value || typeof value !== 'object') return null;
  return value as CollabAwarenessState;
}

function serverStateOf(state: Record<string, unknown> | null): {
  recoveryError?: { documentId?: unknown; message?: unknown };
} | null {
  const value = state?.quarryServer;
  if (!value || typeof value !== 'object') return null;
  return value as { recoveryError?: { documentId?: unknown; message?: unknown } };
}
