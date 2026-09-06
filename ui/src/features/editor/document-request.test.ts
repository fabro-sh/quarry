import { documentRequest } from './document-request';

afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

test.each(['headers', 'body'])('a request deadline covers stalled %s and aborts only the attempt', async (stage) => {
  vi.useFakeTimers();
  let signal: AbortSignal | undefined;
  vi.stubGlobal('fetch', vi.fn((_url, options) => {
    signal = options.signal;
    if (stage === 'headers') return new Promise(() => {});
    return Promise.resolve(new Response(new ReadableStream({ start() {} })));
  }));
  const result = documentRequest('/commands', { request_id: 'unchanged' }, undefined, 100).catch((error) => error);
  await vi.advanceTimersByTimeAsync(100);
  expect((await result).message).toBe('The document request timed out');
  expect(signal?.aborted).toBe(true); expect(vi.getTimerCount()).toBe(0);
});

test('navigation can end an attempt even when the fetch implementation ignores cancellation', async () => {
  vi.useFakeTimers();
  vi.stubGlobal('fetch', vi.fn(() => new Promise(() => {})));
  const controller = new AbortController();
  const result = documentRequest('/commands', { request_id: 'persisted' }, controller.signal).catch((error) => error);
  controller.abort(new Error('Page suspended'));
  expect((await result).message).toBe('Page suspended'); expect(vi.getTimerCount()).toBe(0);
});

test('an invalid successful response cannot acknowledge an edit', async () => {
  vi.stubGlobal('fetch', vi.fn(async () => new Response('not JSON', { status: 200 })));
  await expect(documentRequest('/commands', { request_id: 'persisted' })).rejects.toThrow('Invalid document response');
});
