import { BrowserEvents } from './browser-events';

class Socket {
  static all: Socket[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: (() => void) | null = null;
  closed = false;
  constructor(readonly url: string) { Socket.all.push(this); }
  close() { this.closed = true; }
}
let sources: BrowserEvents[];
beforeEach(() => {
  vi.useFakeTimers(); vi.stubGlobal('WebSocket', Socket); Socket.all = []; sources = [];
});
afterEach(() => {
  sources.forEach((source) => source.close()); vi.useRealTimers(); vi.unstubAllGlobals();
});
async function open(url = '/v1/events?library=notes') {
  const source = new BrowserEvents(url); sources.push(source);
  await vi.advanceTimersByTimeAsync(0);
  return source;
}

test('notifications keep their scope and event type without opening an HTTP event stream', async () => {
  const eventSource = vi.fn(); vi.stubGlobal('EventSource', eventSource);
  const source = await open('https://example.test/v1/libraries/notes/documents-by-id/doc/events/stream?token=invitation');
  const changed = vi.fn(), opened = vi.fn(), failed = vi.fn();
  source.addEventListener('doc.changed', changed); source.onopen = opened; source.onerror = failed;
  const socket = Socket.all[0];
  expect(socket.url).toBe('wss://example.test/v1/libraries/notes/documents-by-id/doc/events/stream?token=invitation');
  socket.onopen!(); expect(opened).toHaveBeenCalledTimes(1);
  for (const data of ['not JSON', '{}', '{"type":"error"}', '{"type":"open"}']) socket.onmessage!(new MessageEvent('message', { data }));
  socket.onmessage!(new MessageEvent('message', { data: '{"type":"doc.changed","doc_id":"doc"}' }));
  expect(changed).toHaveBeenCalledTimes(1);
  expect(JSON.parse(changed.mock.calls[0][0].data)).toEqual({ type: 'doc.changed', doc_id: 'doc' });
  expect(failed).not.toHaveBeenCalled(); expect(eventSource).not.toHaveBeenCalled();
});

test('a failed or stalled connection reports unavailability and reconnects with fresh events', async () => {
  const source = await open(); const errors = vi.fn(), opened = vi.fn(), changed = vi.fn();
  source.onerror = errors; source.onopen = opened; source.addEventListener('doc.changed', changed);
  const first = Socket.all[0], oldMessage = first.onmessage!;
  await vi.advanceTimersByTimeAsync(5000);
  expect(first.closed).toBe(true); expect(errors).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(3000);
  expect(Socket.all).toHaveLength(2); Socket.all[1].onopen!();
  oldMessage(new MessageEvent('message', { data: '{"type":"doc.changed"}' }));
  expect(changed).not.toHaveBeenCalled(); expect(opened).toHaveBeenCalledTimes(1);
  Socket.all[1].onclose!(); expect(errors).toHaveBeenCalledTimes(2);
  source.close(); await vi.advanceTimersByTimeAsync(30000);
  expect(Socket.all).toHaveLength(2);
});

test('page suspension closes the socket and restoration reconnects without stale callbacks', async () => {
  const source = await open(); const changed = vi.fn(); source.addEventListener('doc.changed', changed);
  const oldMessage = Socket.all[0].onmessage!;
  window.dispatchEvent(new Event('pagehide'));
  expect(Socket.all[0].closed).toBe(true);
  await vi.advanceTimersByTimeAsync(30000); expect(Socket.all).toHaveLength(1);
  window.dispatchEvent(new Event('pageshow')); expect(Socket.all).toHaveLength(2);
  oldMessage(new MessageEvent('message', { data: '{"type":"doc.changed"}' }));
  expect(changed).not.toHaveBeenCalled();
  source.close(); window.dispatchEvent(new Event('pageshow'));
  expect(Socket.all[1].closed).toBe(true); expect(Socket.all).toHaveLength(2);
});

test('missing socket support reports fallback availability without using EventSource', async () => {
  vi.stubGlobal('WebSocket', undefined);
  const source = new BrowserEvents('/v1/events?library=notes'); sources.push(source);
  const failed = vi.fn(); source.onerror = failed;
  await vi.advanceTimersByTimeAsync(0); expect(failed).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(3000); expect(failed).toHaveBeenCalledTimes(2);
  source.close(); await vi.advanceTimersByTimeAsync(30000); expect(failed).toHaveBeenCalledTimes(2);
});

test('workspace and document interests share one library connection with independent filtering and lifetime', async () => {
  const workspace = await open('/v1/events?library=notes');
  const first = await open('/v1/libraries/notes/documents-by-id/first/events/stream');
  const second = await open('/v1/libraries/notes/documents-by-id/second/events/stream');
  expect(Socket.all).toHaveLength(1);
  const workspaceChanged = vi.fn(), firstChanged = vi.fn(), secondChanged = vi.fn(), lagged = vi.fn();
  workspace.addEventListener('doc.changed', workspaceChanged);
  first.addEventListener('doc.changed', firstChanged); second.addEventListener('doc.changed', secondChanged);
  first.addEventListener('stream.lagged', lagged);
  Socket.all[0].onmessage!(new MessageEvent('message', { data: '{"type":"doc.changed","doc_id":"first"}' }));
  expect(workspaceChanged).toHaveBeenCalledTimes(1); expect(firstChanged).toHaveBeenCalledTimes(1); expect(secondChanged).not.toHaveBeenCalled();
  workspace.close(); first.close(); expect(Socket.all[0].closed).toBe(false);
  Socket.all[0].onmessage!(new MessageEvent('message', { data: '{"type":"doc.changed","doc_id":"second"}' }));
  expect(secondChanged).toHaveBeenCalledTimes(1); expect(workspaceChanged).toHaveBeenCalledTimes(1);
  second.close(); expect(Socket.all[0].closed).toBe(true);
});

test('temporary subscriptions share their capability while invitations and origins stay isolated', async () => {
  const tmp = await open('/v1/tmp/documents/capability/events/stream');
  const duplicate = await open('/v1/tmp/documents/capability/events/stream');
  await open('/v1/tmp/documents/another/events/stream');
  await open('/v1/libraries/notes/documents-by-id/first/events/stream?token=viewer');
  await open('/v1/libraries/notes/documents-by-id/first/events/stream?token=editor');
  await open('https://elsewhere.test/v1/tmp/documents/capability/events/stream');
  expect(Socket.all).toHaveLength(5);
  expect(Socket.all[2].url).toContain('documents-by-id/first/events/stream?token=viewer');
  tmp.close(); expect(Socket.all[0].closed).toBe(false);
  duplicate.close(); expect(Socket.all[0].closed).toBe(true);
});

test('late consumers observe connectivity and document consumers receive lag notifications', async () => {
  await open('/v1/events?library=notes'); Socket.all[0].onopen!();
  const late = new BrowserEvents('/v1/libraries/notes/documents-by-id/first/events/stream'); sources.push(late);
  const opened = vi.fn(), lagged = vi.fn(); late.onopen = opened; late.addEventListener('stream.lagged', lagged);
  await vi.advanceTimersByTimeAsync(0); expect(opened).toHaveBeenCalledTimes(1);
  Socket.all[0].onmessage!(new MessageEvent('message', { data: '{"type":"stream.lagged"}' }));
  expect(lagged).toHaveBeenCalledTimes(1);
});
