// Toggleable collaboration-lifecycle logging. The collab path (SSE event
// classification → session updates → reconnect probes) is hard to observe
// after the fact, so this channel emits structured `[collab]` console debug
// lines at each decision point. It is a no-op unless explicitly enabled:
//
//   - add `?collabDebug` (or `?collabDebug=1`) to the URL, or
//   - set `localStorage['quarry:collabDebug'] = '1'`.
//
// Enabling via the URL is the easiest way to capture a session for a bug report.

const STORAGE_KEY = 'quarry:collabDebug';

/** Pure enablement decision. The URL query param, when present, wins over the
 *  stored flag; `0`/`false` explicitly disable. */
export function collabDebugEnabledFrom(search: string, stored: string | null): boolean {
  const params = new URLSearchParams(search);
  if (params.has('collabDebug')) {
    const value = params.get('collabDebug');
    return value !== '0' && value !== 'false';
  }
  return stored === '1' || stored === 'true';
}

let cached: boolean | null = null;

export type CollabLifecycleEvent =
  | 'editor_disposed'
  | 'editor_mounted'
  | 'mirror_completed'
  | 'mirror_scheduled'
  | 'probe_attempted'
  | 'provider_created'
  | 'provider_destroyed'
  | 'session_epoch_started';

const lifecycleCounters = new Map<CollabLifecycleEvent, number>();

function enabled(): boolean {
  if (cached !== null) return cached;
  cached = readEnabled();
  return cached;
}

function readEnabled(): boolean {
  if (typeof window === 'undefined') return false;
  try {
    return collabDebugEnabledFrom(
      window.location.search,
      window.localStorage?.getItem(STORAGE_KEY) ?? null
    );
  } catch {
    return false;
  }
}

/** Emit a `[collab] <event>` debug line with structured detail when enabled. */
export function collabDebug(event: string, detail?: Record<string, unknown>): void {
  if (!enabled()) return;
  // eslint-disable-next-line no-console
  console.debug(`[collab] ${event}`, detail ?? {});
}

/** Low-cost lifecycle counters used by regression tests and opt-in debug logs. */
export function recordCollabLifecycleEvent(event: CollabLifecycleEvent): void {
  const count = (lifecycleCounters.get(event) ?? 0) + 1;
  lifecycleCounters.set(event, count);
  collabDebug(`lifecycle.${event}`, { count });
}

export function collabLifecycleSnapshot(): Readonly<Record<CollabLifecycleEvent, number>> {
  return {
    editor_disposed: lifecycleCounters.get('editor_disposed') ?? 0,
    editor_mounted: lifecycleCounters.get('editor_mounted') ?? 0,
    mirror_completed: lifecycleCounters.get('mirror_completed') ?? 0,
    mirror_scheduled: lifecycleCounters.get('mirror_scheduled') ?? 0,
    probe_attempted: lifecycleCounters.get('probe_attempted') ?? 0,
    provider_created: lifecycleCounters.get('provider_created') ?? 0,
    provider_destroyed: lifecycleCounters.get('provider_destroyed') ?? 0,
    session_epoch_started: lifecycleCounters.get('session_epoch_started') ?? 0,
  };
}

export function resetCollabLifecycleCounters(): void {
  lifecycleCounters.clear();
}
