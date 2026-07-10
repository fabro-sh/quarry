import { useEffect, useMemo, useSyncExternalStore } from 'react';

import {
  CollabEditorSession,
  type CollabSessionSnapshot,
} from './collab-editor-session';
import type { CollabSaveState } from './save-state';

interface UseCollabEditorSessionOptions {
  readonly baseUrl: string;
  readonly documentId: string;
  readonly enabled: boolean;
  readonly onSaveStateChange?: (state: CollabSaveState) => void;
  readonly roomName: string;
}

export interface CollabEditorSessionValue {
  readonly session: CollabEditorSession;
  readonly snapshot: CollabSessionSnapshot;
}

export function useCollabEditorSession({
  baseUrl,
  documentId,
  enabled,
  onSaveStateChange,
  roomName,
}: UseCollabEditorSessionOptions): CollabEditorSessionValue {
  const session = useMemo(
    () =>
      new CollabEditorSession({
        baseUrl,
        enabled,
        roomName,
      }),
    [baseUrl, documentId, enabled, roomName]
  );
  const snapshot = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot
  );

  useCollabSessionLifetime(session);
  useCollabSaveStatePublisher(snapshot.saveState, onSaveStateChange);

  return { session, snapshot };
}

function useCollabSessionLifetime(session: CollabEditorSession): void {
  useEffect(() => {
    session.start();
    return () => session.suspend();
  }, [session]);
}

function useCollabSaveStatePublisher(
  saveState: CollabSaveState | null,
  onSaveStateChange: ((state: CollabSaveState) => void) | undefined
): void {
  useEffect(() => {
    if (saveState) onSaveStateChange?.(saveState);
  }, [onSaveStateChange, saveState]);
}
