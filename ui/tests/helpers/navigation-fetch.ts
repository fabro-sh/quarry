import type { FetchEvent, FetchRead } from './navigation-errors';

/** Runs before application code; observes fetch without consuming rejections. */
export function installNavigationFetchTracking(options: { origin: string; readPath: string; binding: string }) {
  if (window.location.origin !== options.origin) return () => {};
  const host = window as unknown as Record<string, (event: FetchEvent) => Promise<void>>;
  const original = window.fetch;
  const active = new Map<string, FetchRead>();
  const now = () => performance.timeOrigin + performance.now();
  const notify = (event: FetchEvent) => {
    try { void host[options.binding](event).catch(() => {}); } catch { /* The test context may already be closing. */ }
  };
  const prepare = () => notify({ type: 'prepare', at: now(), reads: [...active.values()] });
  window.addEventListener('beforeunload', prepare);
  window.fetch = function(input, init) {
    const request = typeof Request !== 'undefined' && input instanceof Request ? input : undefined;
    const signal = init?.signal ?? request?.signal;
    const method = (init?.method ?? request?.method ?? 'GET').toUpperCase();
    if (method !== 'GET' || !signal || signal.aborted) return original.call(this, input, init);
    let url: URL;
    try { url = new URL(request?.url ?? String(input), window.location.href); }
    catch { return original.call(this, input, init); }
    if (url.origin !== options.origin || !new RegExp(options.readPath).test(url.pathname)) {
      return original.call(this, input, init);
    }
    const read = { id: crypto.randomUUID(), url: url.href, at: now() };
    active.set(read.id, read); notify({ type: 'start', ...read });
    const abort = () => notify({ type: 'abort', id: read.id, at: now() });
    signal.addEventListener('abort', abort, { once: true });
    const finish = (result: 'fulfilled' | 'rejected') => {
      active.delete(read.id); signal.removeEventListener('abort', abort);
      notify({ type: 'finish', id: read.id, at: now(), result });
    };
    try {
      // Return this rejecting chain to the caller. A side catch on the original
      // promise would hide an unhandled rejection from the browser test.
      return original.call(this, input, init).then((response) => {
        finish('fulfilled'); return response;
      }, (error) => { finish('rejected'); throw error; });
    } catch (error) { finish('rejected'); throw error; }
  };
  return () => { window.fetch = original; window.removeEventListener('beforeunload', prepare); };
}
