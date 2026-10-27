import { expect, test } from 'playwright/test';
import { NavigationErrors } from './helpers/navigation-errors';

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
