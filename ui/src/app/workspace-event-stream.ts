import { useEffect, useRef } from 'react';
import { subscribeBrowserEvents } from '../lib/browser-events';

export interface BrowserEventPayload {
  readonly type: string;
  readonly path?: string | null;
  readonly from?: string | null;
  readonly to?: string | null;
  readonly doc_id?: string | null;
  readonly version_id?: string | null;
  readonly etag?: string | null;
  readonly origin_id?: string | null;
  readonly source?: string | null;
  readonly tx_id?: string | null;
  readonly peer_id?: string | null;
  readonly applied?: number | null;
  readonly conflicts?: number | null;
}

export type EventStreamState = 'idle' | 'connecting' | 'open' | 'polling';

interface WorkspaceEventStreamOptions {
  readonly enabled: boolean;
  readonly eventTypes: readonly string[];
  readonly onEvent: (payload: BrowserEventPayload) => void;
  readonly onPoll: () => void;
  readonly onStateChange?: (state: EventStreamState) => void;
  readonly pollIntervalMs: number;
  readonly url: string;
}

/** Owns notification and polling-fallback lifetime while callbacks stay current. */
export function useWorkspaceEventStream({
  enabled,
  eventTypes,
  onEvent,
  onPoll,
  onStateChange,
  pollIntervalMs,
  url,
}: WorkspaceEventStreamOptions): void {
  const onEventRef = useLatest(onEvent);
  const onPollRef = useLatest(onPoll);
  const onStateChangeRef = useLatest(onStateChange);

  useEffect(() => {
    if (!enabled) {
      onStateChangeRef.current?.('idle');
      return;
    }
    let pollingTimer: ReturnType<typeof setInterval> | null = null;

    const poll = () => onPollRef.current();
    const startPolling = () => {
      onStateChangeRef.current?.('polling');
      poll();
      pollingTimer ??= setInterval(poll, pollIntervalMs);
    };
    const stopPolling = () => {
      if (!pollingTimer) return;
      clearInterval(pollingTimer);
      pollingTimer = null;
    };

    onStateChangeRef.current?.('connecting');
    const source = subscribeBrowserEvents(url);
    const handleEvent = (event: Event) => {
      const payload = parseBrowserEvent(event as MessageEvent);
      if (payload) onEventRef.current(payload);
    };
    for (const eventType of eventTypes) source.addEventListener(eventType, handleEvent);
    source.onopen = () => {
      // Reconnect and periodic reads recover silently missed notifications.
      poll();
      pollingTimer ??= setInterval(poll, pollIntervalMs);
      onStateChangeRef.current?.('open');
    };
    source.onerror = startPolling;

    return () => {
      for (const eventType of eventTypes) source.removeEventListener(eventType, handleEvent);
      source.close();
      stopPolling();
    };
  }, [enabled, eventTypes, onEventRef, onPollRef, onStateChangeRef, pollIntervalMs, url]);
}

function useLatest<T>(value: T): { current: T } {
  const ref = useRef(value);
  ref.current = value;
  return ref;
}

function parseBrowserEvent(event: MessageEvent): BrowserEventPayload | null {
  try {
    const payload = JSON.parse(String(event.data)) as BrowserEventPayload;
    return typeof payload.type === 'string' ? payload : null;
  } catch {
    return null;
  }
}
