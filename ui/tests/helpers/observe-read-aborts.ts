import type { ReadAbortEvent } from './navigation-errors';

/** Observe signals only. Return the exact native promise without handling it. */
export function observeReadAborts(options: { origin: string; marker: string }) {
  if (location.origin !== options.origin) return;
  const nativeFetch = window.fetch;
  const report = console.debug.bind(console);
  const notify = (event: ReadAbortEvent) => {
    // Console events are emitted synchronously even while the realm unloads.
    // An exposed async binding can be discarded when that realm is destroyed.
    report(options.marker + JSON.stringify(event));
  };
  window.fetch = function(input, init) {
    const request = input instanceof Request ? input : undefined;
    const signal = init?.signal ?? request?.signal;
    const method = (init?.method ?? request?.method ?? 'GET').toUpperCase();
    if (method !== 'GET' || !signal || signal.aborted) return nativeFetch.call(this, input, init);
    let url: URL;
    try { url = new URL(request?.url ?? String(input), location.href); }
    catch { return nativeFetch.call(this, input, init); }
    if (url.origin !== options.origin || !/^\/v1\/(?:libraries\/[^/]+\/documents-by-id|tmp\/documents)\/[^/]+\/document-state$/.test(url.pathname)) {
      return nativeFetch.call(this, input, init);
    }
    const event = { id: crypto.randomUUID(), url: url.href };
    notify({ ...event, phase: 'start' });
    const abort = () => notify({ ...event, phase: 'abort' });
    signal.addEventListener('abort', abort, { once: true });
    try { return nativeFetch.call(this, input, init); }
    catch (error) { signal.removeEventListener('abort', abort); throw error; }
  };
}
