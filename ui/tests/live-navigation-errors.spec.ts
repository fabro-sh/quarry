import { expect, test } from 'playwright/test';
import { NavigationErrors } from './helpers/navigation-errors';

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
