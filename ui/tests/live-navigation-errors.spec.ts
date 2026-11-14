import { expect, test } from 'playwright/test';
import { NavigationErrors } from './helpers/navigation-errors';
import { observeReadAborts } from './helpers/observe-read-aborts';
import { uiOrigin } from './helpers/native-document';

const origin = 'http://127.0.0.1:5174';
const url = `${origin}/v1/libraries/example/documents-by-id/document/document-state?since=abc`;
const diagnostic = (target = url, source = origin, caller = 'refresh') => ({ message: `Cannot load ${target}`, stack: `Fetch API cannot load ${target} due to access control checks.\n    at documentRequest (${source}/src/features/editor/document-request.ts:17:35)\n    at ${caller} (${source}/src/features/editor/document-session.ts?t=123:256:40)` });
function fixture(browser = 'webkit', target = url, method = 'GET', fetch = true) {
  const tracker = new NavigationErrors(browser, origin), request = {}, errors: string[] = [];
  tracker.requestStarted(request, target, method, fetch);
  return { tracker, request, errors };
}

for (const caller of ['open', 'refresh']) test(`a proven handled ${caller} cancellation is ignored only once`, () => {
  const { tracker, request, errors } = fixture();
  tracker.requestFailed(request, 'cancelled', true);
  tracker.record(diagnostic(url, origin, caller), errors);
  expect(errors).toEqual([]);
  tracker.record(diagnostic(url, origin, caller), errors);
  expect(errors).toHaveLength(1);
});

test('an error reported before network cancellation remains visible', () => {
  const { tracker, request, errors } = fixture();
  tracker.record(diagnostic(), errors); tracker.requestFailed(request, 'cancelled', true);
  expect(errors).toHaveLength(1);
});

for (const reason of ['Access control check failed', 'connection reset']) test(`non-cancellation network failure remains visible: ${reason}`, () => {
  const { tracker, request, errors } = fixture();
  tracker.requestFailed(request, reason, true); tracker.record(diagnostic(), errors);
  expect(errors).toHaveLength(1);
});

test('uncancelled and successfully completed reads remain errors', () => {
  const { tracker, request, errors } = fixture();
  tracker.record(diagnostic(), errors); tracker.requestFinished(request); tracker.record(diagnostic(), errors);
  expect(errors).toHaveLength(2);
});

test('a cross-origin redirect cannot borrow a same-origin cancellation', () => {
  const { tracker, request, errors } = fixture();
  tracker.requestFailed(request, 'cancelled', false); tracker.record(diagnostic(), errors);
  expect(errors).toHaveLength(1);
});

test('concurrent requests to the same URL cannot be mistaken for one request', () => {
  const { tracker, request, errors } = fixture(); const other = {};
  tracker.requestStarted(other, url, 'GET', true);
  tracker.requestFailed(request, 'cancelled', true); tracker.requestFailed(other, 'cancelled', true);
  tracker.record(diagnostic(), errors);
  expect(errors).toHaveLength(1);
});

test('a later request clears an earlier cancellation proof', () => {
  const { tracker, request, errors } = fixture();
  tracker.requestFailed(request, 'cancelled', true); tracker.requestStarted({}, url, 'GET', true);
  tracker.record(diagnostic(), errors);
  expect(errors).toHaveLength(1);
});

for (const target of [url.replace(origin, 'http://foreign.example'), `${origin}/v1/unrelated`]) test(`foreign and unrelated reads remain visible: ${target}`, () => {
  const { tracker, request, errors } = fixture('webkit', target);
  tracker.requestFailed(request, 'cancelled', true); tracker.record(diagnostic(target), errors);
  expect(errors).toHaveLength(1);
});

for (const [method, fetch] of [['POST', true], ['GET', false]] as const) test(`only GET fetch reads qualify (${method}, ${fetch})`, () => {
  const { tracker, request, errors } = fixture('webkit', url, method, fetch);
  tracker.requestFailed(request, 'cancelled', true); tracker.record(diagnostic(), errors);
  expect(errors).toHaveLength(1);
});

test('a cancellation for another full URL cannot hide an error', () => {
  const { tracker, request, errors } = fixture();
  tracker.requestFailed(request, 'cancelled', true); tracker.record(diagnostic(`${url}different`), errors);
  expect(errors).toHaveLength(1);
});

test('arbitrary callers and foreign source frames cannot use a session cancellation proof', () => {
  const { tracker, request, errors } = fixture();
  tracker.requestFailed(request, 'cancelled', true);
  tracker.record(diagnostic(url, origin, 'arbitraryCaller'), errors);
  tracker.record(diagnostic(url, 'http://foreign.example'), errors);
  tracker.record({ message: 'No stack' }, errors);
  expect(errors).toHaveLength(3);
});

for (const browser of ['firefox', 'chromium']) test(`${browser} errors are never filtered`, () => {
  const { tracker, request, errors } = fixture(browser);
  tracker.requestFailed(request, 'cancelled', true); tracker.record(diagnostic(), errors);
  expect(errors).toHaveLength(1);
});

test('an observed signal abort proves a pre-network handled cancellation only once', () => {
  const tracker = new NavigationErrors('webkit', origin), errors: string[] = [];
  tracker.observeRead({ phase: 'start', id: 'read', url });
  tracker.observeRead({ phase: 'abort', id: 'read', url });
  tracker.record(diagnostic(), errors);
  expect(errors).toEqual([]);
  tracker.record(diagnostic(), errors);
  expect(errors).toHaveLength(1);
});

test('stale signal aborts and foreign redirects cannot supply cancellation proof', () => {
  const tracker = new NavigationErrors('webkit', origin), errors: string[] = [];
  tracker.observeRead({ phase: 'start', id: 'old', url });
  tracker.observeRead({ phase: 'start', id: 'new', url });
  tracker.observeRead({ phase: 'abort', id: 'old', url });
  tracker.record(diagnostic(), errors);
  tracker.foreignRedirect(url);
  tracker.observeRead({ phase: 'abort', id: 'new', url });
  tracker.record(diagnostic(), errors);
  expect(errors).toHaveLength(2);
});

test('old-document handled reads require completed navigation, and new-page errors remain visible', () => {
  const tracker = new NavigationErrors('webkit', origin), errors: string[] = [];
  tracker.observeRead({ phase: 'start', id: 'old', url });
  tracker.observeRead({ phase: 'closing', reads: [{ id: 'old', url }] });
  tracker.record(diagnostic(), errors);
  expect(errors).toHaveLength(1);
  tracker.observeRead({ phase: 'start', id: 'new', url });
  tracker.record(diagnostic(), errors);
  tracker.record({ message: 'Application exception' }, errors);
  tracker.navigationCompleted();
  expect(errors).toEqual([diagnostic().message, 'Application exception']);
});

test('a cancelled navigation cannot discard a real error before a later navigation', () => {
  const tracker = new NavigationErrors('webkit', origin), errors: string[] = [];
  tracker.observeRead({ phase: 'start', id: 'old', url });
  tracker.observeRead({ phase: 'closing', reads: [{ id: 'old', url }] });
  tracker.record(diagnostic(), errors);
  tracker.observeRead({ phase: 'closing', reads: [{ id: 'old', url }] });
  tracker.navigationCompleted();
  expect(errors).toHaveLength(1);
});

test('a foreign redirect discovered after the diagnostic keeps the error visible', () => {
  const tracker = new NavigationErrors('webkit', origin), errors: string[] = [];
  tracker.observeRead({ phase: 'start', id: 'old', url });
  tracker.observeRead({ phase: 'closing', reads: [{ id: 'old', url }] });
  tracker.record(diagnostic(), errors);
  tracker.foreignRedirect(url);
  tracker.observeRead({ phase: 'start', id: 'new-page', url });
  tracker.navigationCompleted();
  expect(errors).toHaveLength(1);
});

test('a queued read in the closing realm is distinct from a new-document read', () => {
  const tracker = new NavigationErrors('webkit', origin), errors: string[] = [];
  tracker.observeRead({ phase: 'closing', reads: [] });
  tracker.observeRead({ phase: 'start', id: 'old-late', url, closing: true });
  tracker.record(diagnostic(), errors);
  tracker.observeRead({ phase: 'start', id: 'new-page', url, closing: false });
  tracker.record(diagnostic(), errors);
  tracker.navigationCompleted();
  expect(errors).toEqual([diagnostic().message]);
});

test('caught SWR metadata reads qualify only while the old document closes', () => {
  const target = `${origin}/v1/libraries/example/documents/doc.md/presence`;
  const error = {
    message: 'Metadata read failed',
    stack: `Fetch API cannot load ${target} due to access control checks.\n    at jsonRequest (${origin}/src/api/client.ts:235:31)\n    at execute (${origin}/node_modules/.vite/deps/swr.js?v=test:981:19)`,
  };
  const tracker = new NavigationErrors('webkit', origin), errors: string[] = [];
  tracker.observeRead({ phase: 'start', id: 'metadata', url: target });
  tracker.record(error, errors);
  expect(errors).toHaveLength(1);
  tracker.observeRead({ phase: 'closing', reads: [{ id: 'metadata', url: target }] });
  tracker.record(error, errors);
  tracker.record({ ...error, stack: error.stack.replace('execute', 'unhandledCaller') }, errors);
  tracker.navigationCompleted();
  expect(errors).toEqual(['Metadata read failed', 'Metadata read failed']);
});

for (const [path, caller] of [
  ['renamed.md', 'getDocument'], ['renamed.md/outgoing-links', 'jsonRequest'],
  ['renamed.md/backlinks', 'jsonRequest'], ['renamed.md/versions', 'jsonRequest'],
  ['renamed.md/presence', 'jsonRequest'], ['renamed.md/review?includeResolved=1', 'jsonRequest'],
] as const) test(`caught initial SWR read during navigation: ${path}`, () => {
  const target = `${origin}/v1/libraries/example/documents/${path}`;
  const stack = `Fetch API cannot load ${target} due to access control checks.
    at unknown (web-inspector://bootstrap.js:623:59)
    at ${caller} (${origin}/src/api/client.ts:235:31)
    at unknown (${origin}/node_modules/.vite/deps/swr.js?v=test:824:27)
    at unknown (${origin}/node_modules/.vite/deps/swr.js?v=test:958:25)`;
  const error = { message: 'Initial metadata read failed', stack };
  const tracker = new NavigationErrors('webkit', origin), errors: string[] = [];
  tracker.observeRead({ phase: 'closing', reads: [] });
  tracker.observeRead({ phase: 'start', id: 'old', url: target, closing: true });
  tracker.record(error, errors);
  tracker.record({ ...error, stack: stack.replace('client.ts', 'unhandled.ts') }, errors);
  tracker.record({ ...error, stack: stack.replaceAll('/node_modules/.vite/deps/swr.js', '/unhandled.js') }, errors);
  tracker.observeRead({ phase: 'start', id: 'new', url: target, closing: false });
  tracker.record(error, errors);
  tracker.navigationCompleted();
  expect(errors).toEqual(Array(3).fill(error.message));
});

test('observing aborts returns the native fetch promise without consuming rejection', async ({ page }) => {
  await page.route(`${uiOrigin}/abort-observer-test`, (route) => route.fulfill({ body: '<!doctype html><title>Observer</title>', contentType: 'text/html' }));
  await page.goto(`${uiOrigin}/abort-observer-test`);
  await page.evaluate(() => {
    const state = window as unknown as { events: unknown[]; result: Promise<Response>; nativeSignal?: AbortSignal };
    state.events = [];
    console.debug = (message: string) => { state.events.push(JSON.parse(message.slice('observer-test:'.length))); };
    state.result = Promise.resolve(new Response('ok'));
    window.fetch = (_input, init) => { state.nativeSignal = init?.signal ?? undefined; return state.result; };
  });
  await page.evaluate(observeReadAborts, { origin: new URL(uiOrigin).origin, marker: 'observer-test:' });
  const result = await page.evaluate(() => {
    const state = window as unknown as { events: Array<{ phase: string }>; result: Promise<Response>; nativeSignal?: AbortSignal };
    const controller = new AbortController();
    const returned = fetch('/v1/libraries/test/documents-by-id/test/document-state', { signal: controller.signal });
    controller.abort();
    return { identical: returned === state.result, signal: state.nativeSignal === controller.signal, events: state.events.map((event) => event.phase) };
  });
  expect(result).toEqual({ identical: true, signal: true, events: ['start', 'abort'] });
  const metadata = await page.evaluate(() => {
    const state = window as unknown as { result: Promise<Response>; events: Array<{ phase: string; reads?: Array<{ url: string }> }> };
    const paths = ['renamed.md', 'renamed.md/outgoing-links', 'renamed.md/backlinks', 'renamed.md/versions', 'renamed.md/presence', 'renamed.md/review?includeResolved=1'];
    const urls = paths.map((path) => `${location.origin}/v1/libraries/test/documents/${path}`);
    const identical = urls.every((url) => fetch(url) === state.result);
    dispatchEvent(new Event('beforeunload'));
    const observed = state.events.at(-1)?.reads?.map((read) => read.url);
    return { identical, covered: urls.every((url) => observed?.includes(url)) };
  });
  expect(metadata).toEqual({ identical: true, covered: true });
  const failures: string[] = [];
  page.on('pageerror', (error) => failures.push(error.message));
  await page.evaluate(() => {
    const state = window as unknown as { result: Promise<Response> };
    state.result = Promise.reject(new Error('unhandled observer regression'));
    void fetch('/v1/libraries/test/documents-by-id/test/document-state', { signal: new AbortController().signal });
  });
  await expect.poll(() => failures.join('\n')).toContain('unhandled observer regression');
});
