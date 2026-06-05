// Toggleable collaboration-lifecycle logging. The collab path (SSE event
// classification → agent-injection adoption → draft/flush → conflict) is hard
// to observe after the fact, so this channel emits structured `[collab]` console
// debug lines at each decision point. It is a no-op unless explicitly enabled:
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
