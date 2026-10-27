import { expect, test } from 'playwright/test';
import { DOCUMENT_READ_PATH, NavigationErrors, type FetchEvent } from './helpers/navigation-errors';
import { installNavigationFetchTracking } from './helpers/navigation-fetch';
import { execFileSync } from 'node:child_process';

const origin = 'http://127.0.0.1:5174';
const error = (url: string) => ({ message: url, stack: `Fetch API cannot load ${url} due to access control checks.` });
const reads = ['/v1/libraries/example/documents', '/v1/libraries/example/documents/doc.md/presence',
  '/v1/libraries/example/documents/folder/doc.md/outgoing-links', '/v1/libraries/example/documents/doc.md/backlinks',
  '/v1/libraries/example/documents-by-id/id/document-state', '/v1/tmp/documents/secret/document-state'];

for (const eventOrder of ['error first', 'network first']) test(`WebKit ignores only proven navigation cancellations: ${eventOrder}`, () => {
  const tracker = new NavigationErrors('webkit', origin);
  const errors: string[] = [];
  const requests = reads.map((path) => { const request = {}; tracker.requestStarted(request, origin + path, 'GET', true); return request; });
  tracker.navigationStarted();
  for (const [index, request] of requests.entries()) {
    if (eventOrder === 'error first') {
      tracker.record(error(origin + reads[index]), errors);
      expect(errors).toEqual([origin + reads[index]]);
      tracker.requestFailed(request, 'cancelled', true);
    } else {
      tracker.requestFailed(request, 'Load request cancelled', true);
      tracker.record(error(origin + reads[index]), errors);
    }
    expect(errors).toEqual([]);
  }
});

test('real failures, writes, new-page reads, foreign redirects and post-load errors remain visible', () => {
  for (const scenario of ['active page', 'new page', 'write', 'non-fetch', 'other endpoint', 'foreign origin', 'foreign redirect', 'network failure', 'successful read', 'after load', 'aborted navigation', 'firefox', 'chromium']) {
    const tracker = new NavigationErrors(scenario === 'firefox' || scenario === 'chromium' ? scenario : 'webkit', origin);
    const errors: string[] = [];
    const request = {};
    const url = scenario === 'foreign origin' ? 'https://other.example' + reads[0] : origin + (scenario === 'other endpoint' ? '/v1/capabilities' : reads[0]);
    if (scenario !== 'new page') tracker.requestStarted(request, url, scenario === 'write' ? 'POST' : 'GET', scenario !== 'non-fetch');
    if (scenario === 'successful read') tracker.requestFinished(request);
    if (scenario !== 'active page') tracker.navigationStarted();
    if (scenario === 'new page') tracker.requestStarted(request, url, 'GET', true);
    if (scenario === 'after load' || scenario === 'aborted navigation') tracker.navigationFinished();
    tracker.requestFailed(request, scenario === 'network failure' ? 'Connection failed' : 'cancelled', scenario !== 'foreign redirect');
    tracker.record(error(url), errors);
    expect(errors, scenario).toEqual([url]);
  }
});

test('unproven page errors and unrelated console errors survive navigation cleanup', () => {
  const tracker = new NavigationErrors('webkit', origin);
  const errors = ['unrelated before'];
  const request = {}; tracker.requestStarted(request, origin + reads[0], 'GET', true); tracker.navigationStarted();
  tracker.record(error(origin + reads[0]), errors);
  errors.push('unrelated after');
  tracker.navigationFinished(); tracker.requestFailed(request, 'cancelled', true);
  expect(errors).toEqual(['unrelated before', origin + reads[0], 'unrelated after']);
});

test('cancellation before the navigation request needs both beforeunload and confirmed navigation', () => {
  const tracker = new NavigationErrors('webkit', origin);
  const errors: string[] = [];
  const request = {}; tracker.requestStarted(request, origin + reads[0], 'GET', true);
  tracker.navigationPrepared();
  tracker.requestFailed(request, 'cancelled', true);
  tracker.record(error(origin + reads[0]), errors);
  expect(errors).toEqual([origin + reads[0]]);
  tracker.navigationStarted();
  expect(errors).toEqual([]);
});

test('a cancelled beforeunload prompt cannot conceal a real error on the still-active page', () => {
  const tracker = new NavigationErrors('webkit', origin);
  const errors: string[] = [];
  const request = {}; tracker.requestStarted(request, origin + reads[0], 'GET', true);
  tracker.navigationPrepared();
  tracker.record(error(origin + reads[0]), errors);
  tracker.requestFailed(request, 'cancelled', true);
  expect(errors).toEqual([origin + reads[0]]);
  tracker.navigationPrepared(); tracker.navigationStarted();
  expect(errors).toEqual([origin + reads[0]]);
});

test('a new request to a cancelled URL keeps its errors even before the next page loads', () => {
  for (const result of ['pending', 'failed', 'completed']) {
    const tracker = new NavigationErrors('webkit', origin);
    const errors: string[] = [];
    const url = origin + reads[0], oldRequest = {}, newRequest = {};
    tracker.requestStarted(oldRequest, url, 'GET', true);
    tracker.navigationPrepared(); tracker.navigationStarted();
    tracker.requestFailed(oldRequest, 'cancelled', true);
    tracker.requestStarted(newRequest, url, 'GET', true);
    if (result === 'failed') tracker.requestFailed(newRequest, 'Connection failed', true);
    if (result === 'completed') tracker.requestFinished(newRequest);
    tracker.record(error(url), errors);
    expect(errors, result).toEqual([url]);
  }
});

for (const order of ['error first', 'signal first']) test(`pre-network reads require signal abort, rejection and completed navigation: ${order}`, () => {
  const tracker = new NavigationErrors('webkit', origin), errors: string[] = [];
  const read = { id: 'read', url: origin + reads[4], at: 100 };
  tracker.fetchEvent({ type: 'start', ...read });
  tracker.fetchEvent({ type: 'prepare', at: 200, reads: [read] });
  if (order === 'error first') tracker.record(error(read.url), errors);
  tracker.fetchEvent({ type: 'abort', id: read.id, at: 201 });
  tracker.fetchEvent({ type: 'finish', id: read.id, at: 202, result: 'rejected' });
  if (order === 'signal first') tracker.record(error(read.url), errors);
  tracker.navigationStarted();
  expect(errors).toEqual([read.url]);
  tracker.navigationFinished();
  expect(errors).toEqual([]);
});

test('new same-URL reads cannot explain earlier errors and do not hide later errors', () => {
  for (const source of ['network', 'JavaScript']) {
    const tracker = new NavigationErrors('webkit', origin), errors: string[] = [];
    const url = origin + reads[4], old = {};
    if (source === 'network') tracker.requestStarted(old, url, 'GET', true, 100);
    else tracker.fetchEvent({ type: 'start', id: 'old', url, at: 100 });
    tracker.navigationPrepared(200); tracker.navigationStarted();
    tracker.record({ message: 'earlier', stack: error(url).stack }, errors);
    tracker.fetchEvent({ type: 'start', id: 'new', url, at: 210 });
    tracker.requestStarted({}, url, 'GET', true, 211);
    tracker.record({ message: 'later', stack: error(url).stack }, errors);
    if (source === 'network') tracker.requestFailed(old, 'cancelled', true);
    else {
      tracker.fetchEvent({ type: 'abort', id: 'old', at: 201 });
      tracker.fetchEvent({ type: 'finish', id: 'old', at: 202, result: 'rejected' });
    }
    tracker.navigationFinished();
    expect(errors, source).toEqual(['later']);
  }
});

test('JavaScript fallback never bypasses real network or redirect evidence', () => {
  for (const timing of [150, undefined]) {
    const tracker = new NavigationErrors('webkit', origin), errors: string[] = [];
    const url = origin + reads[4], network = {};
    tracker.fetchEvent({ type: 'start', id: 'read', url, at: 100 });
    tracker.requestStarted(network, url, 'GET', true, timing);
    tracker.navigationPrepared(200); tracker.navigationStarted();
    tracker.fetchEvent({ type: 'abort', id: 'read', at: 201 });
    tracker.fetchEvent({ type: 'finish', id: 'read', at: 202, result: 'rejected' });
    tracker.requestFailed(network, 'cancelled', false);
    tracker.record(error(url), errors); tracker.navigationFinished();
    expect(errors).toEqual([url]);
  }
});

test('uncertain JavaScript reads retain their errors', () => {
  for (const scenario of ['active page', 'not aborted', 'fulfilled', 'aborted early', 'multiple reads', 'new read', 'cancelled navigation', 'foreign origin', 'unknown endpoint']) {
    const tracker = new NavigationErrors('webkit', origin), errors: string[] = [];
    const url = scenario === 'foreign origin' ? 'https://other.example' + reads[4] : origin + (scenario === 'unknown endpoint' ? '/other' : reads[4]);
    tracker.fetchEvent({ type: 'start', id: 'read', url, at: scenario === 'new read' ? 210 : 100 });
    if (scenario === 'multiple reads') tracker.fetchEvent({ type: 'start', id: 'second', url, at: 110 });
    if (scenario !== 'active page') { tracker.navigationPrepared(200); tracker.navigationStarted(); }
    if (scenario !== 'not aborted') tracker.fetchEvent({ type: 'abort', id: 'read', at: scenario === 'aborted early' ? 150 : 220 });
    tracker.fetchEvent({ type: 'finish', id: 'read', at: 230, result: scenario === 'fulfilled' ? 'fulfilled' : 'rejected' });
    tracker.record(error(url), errors);
    tracker.navigationFinished(scenario !== 'cancelled navigation');
    expect(errors, scenario).toEqual([url]);
  }
});

function fakeWindow(fetch: typeof globalThis.fetch, events: FetchEvent[]) {
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'window');
  const target = Object.assign(new EventTarget(), { fetch, location: { origin, href: origin + '/' },
    observe: async (event: FetchEvent) => { events.push(event); } });
  Object.defineProperty(globalThis, 'window', { configurable: true, value: target });
  return { target, restore() { if (previous) Object.defineProperty(globalThis, 'window', previous); else Reflect.deleteProperty(globalThis, 'window'); } };
}

test('fetch instrumentation preserves responses, rejection and request arguments', async () => {
  const events: FetchEvent[] = [], calls: unknown[][] = [], failure = new Error('active rejection');
  let reject = false;
  const response = new Response('ok');
  const fake = fakeWindow(((...args: unknown[]) => { calls.push(args); return reject ? Promise.reject(failure) : Promise.resolve(response); }) as typeof fetch, events);
  const restore = installNavigationFetchTracking({ origin, readPath: DOCUMENT_READ_PATH, binding: 'observe' });
  const init = { signal: new AbortController().signal };
  try {
    expect(await window.fetch(origin + reads[4], init)).toBe(response);
    reject = true;
    await expect(window.fetch(origin + reads[4], init)).rejects.toBe(failure);
    expect(calls).toEqual([[origin + reads[4], init], [origin + reads[4], init]]);
    expect(events.filter((event) => event.type === 'finish').map((event) => event.result)).toEqual(['fulfilled', 'rejected']);
  } finally { restore(); fake.restore(); }
});

test('fetch instrumentation records actual signal aborts and pending reads at beforeunload', async () => {
  const events: FetchEvent[] = [];
  const fake = fakeWindow(((_input, init) => new Promise((_resolve, reject) => {
    init!.signal!.addEventListener('abort', () => reject(init!.signal!.reason), { once: true });
  })) as typeof fetch, events);
  const restore = installNavigationFetchTracking({ origin, readPath: DOCUMENT_READ_PATH, binding: 'observe' });
  try {
    const controller = new AbortController(), promise = window.fetch(origin + reads[4], { signal: controller.signal });
    fake.target.dispatchEvent(new Event('beforeunload'));
    const failure = new Error('cancel read'); controller.abort(failure);
    await expect(promise).rejects.toBe(failure);
    expect(events.map((event) => event.type)).toEqual(['start', 'prepare', 'abort', 'finish']);
    const start = events[0] as Extract<FetchEvent, { type: 'start' }>;
    expect((events[1] as Extract<FetchEvent, { type: 'prepare' }>).reads).toEqual([{ id: start.id, url: start.url, at: start.at }]);
  } finally { restore(); fake.restore(); }
});

test('unhandled active-page fetch rejection remains unhandled after instrumentation', () => {
  const script = `
    const install = ${installNavigationFetchTracking.toString()};
    globalThis.window = Object.assign(new EventTarget(), {
      location: { origin: ${JSON.stringify(origin)}, href: ${JSON.stringify(origin + '/')} },
      fetch: () => Promise.reject(new Error('unhandled active rejection')), observe: async () => {},
    });
    process.once('unhandledRejection', error => { console.log(error.message); process.exit(0); });
    install(${JSON.stringify({ origin, readPath: DOCUMENT_READ_PATH, binding: 'observe' })});
    window.fetch(${JSON.stringify(origin + reads[4])}, { signal: new AbortController().signal });
    setTimeout(() => process.exit(2), 1000);
  `;
  expect(execFileSync('bun', ['--eval', script], { encoding: 'utf8' }).trim()).toBe('unhandled active rejection');
});

test('unknown network timing stays conservative while resolved later-read timing preserves historical errors', () => {
  for (const resolvedTime of [undefined, 150, 220]) {
    const tracker = new NavigationErrors('webkit', origin), errors: string[] = [];
    const url = origin + reads[4];
    let at: number | undefined;
    tracker.fetchEvent({ type: 'start', id: 'old', url, at: 100 });
    tracker.navigationPrepared(200); tracker.navigationStarted();
    tracker.record(error(url), errors);
    tracker.requestStarted({}, url, 'GET', true, () => at);
    tracker.fetchEvent({ type: 'abort', id: 'old', at: 201 });
    tracker.fetchEvent({ type: 'finish', id: 'old', at: 202, result: 'rejected' });
    at = resolvedTime; tracker.navigationFinished();
    expect(errors).toEqual(resolvedTime === 220 ? [] : [url]);
  }
});

test('fetch instrumentation leaves unrelated, signal-less and already-aborted requests untouched', async () => {
  const events: FetchEvent[] = [];
  const fake = fakeWindow((async () => new Response('ok')) as typeof fetch, events);
  const restore = installNavigationFetchTracking({ origin, readPath: DOCUMENT_READ_PATH, binding: 'observe' });
  try {
    const signal = new AbortController().signal, stopped = new AbortController(); stopped.abort();
    await window.fetch(origin + '/unknown', { signal });
    await window.fetch('https://other.example' + reads[4], { signal });
    await window.fetch(origin + reads[4]);
    await window.fetch(origin + reads[4], { signal, method: 'POST' });
    await window.fetch(origin + reads[4], { signal: stopped.signal });
    expect(events).toEqual([]);
  } finally { restore(); fake.restore(); }
});


test('fetch instrumentation delegates invalid URLs without changing native rejection', async () => {
  const events: FetchEvent[] = [], calls: unknown[][] = [], failure = new TypeError('Invalid URL');
  const fake = fakeWindow(((...args: unknown[]) => { calls.push(args); return Promise.reject(failure); }) as typeof fetch, events);
  const restore = installNavigationFetchTracking({ origin, readPath: DOCUMENT_READ_PATH, binding: 'observe' });
  try {
    const init = { signal: new AbortController().signal };
    await expect(window.fetch('http://[')).rejects.toBe(failure);
    await expect(window.fetch('http://[', init)).rejects.toBe(failure);
    expect(calls).toEqual([['http://[', undefined], ['http://[', init]]);
    expect(events).toEqual([]);
  } finally { restore(); fake.restore(); }
});

test('fetch instrumentation preserves synchronous thrown error identity', () => {
  const events: FetchEvent[] = [], failure = new Error('custom fetch threw');
  const fake = fakeWindow((() => { throw failure; }) as typeof fetch, events);
  const restore = installNavigationFetchTracking({ origin, readPath: DOCUMENT_READ_PATH, binding: 'observe' });
  try {
    expect(() => window.fetch(origin + reads[4], { signal: new AbortController().signal })).toThrow(failure);
    expect(events.map((event) => event.type)).toEqual(['start', 'finish']);
  } finally { restore(); fake.restore(); }
});
