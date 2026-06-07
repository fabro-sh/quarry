import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { SWRConfig, type SWRConfiguration } from 'swr';

import type { CollabEditorConfig } from '../features/editor/MarkdownEditor';
import { App } from './App';

type TestWindow = Window & {
  __quarryTestCollab?: CollabEditorConfig;
};

function testWindow() {
  return window as TestWindow;
}

vi.mock('../features/editor/MarkdownEditor', () => ({
  MarkdownEditor({
    collab,
    content,
    onChange,
  }: {
    collab?: CollabEditorConfig;
    content: string;
    onChange: (content: string) => void;
  }) {
    testWindow().__quarryTestCollab = collab;
    return (
      <textarea
        aria-label="Plate markdown editor"
        onChange={(event) => onChange(event.currentTarget.value)}
        value={content}
      />
    );
  },
}));

describe('workspace document mutation provenance', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    localStorage.clear();
    delete testWindow().__quarryTestCollab;
    window.history.pushState({}, '', '/');
  });

  it('stamps toolbar deletes with the browser origin, clears selection, and ignores its own delete echo', async () => {
    stubBrowserOrigin('00000000-0000-4000-8000-000000000001');
    const fetch = vi.fn(provenanceFetch());
    vi.stubGlobal('fetch', fetch);
    vi.stubGlobal('EventSource', MockEventSource);
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    MockEventSource.instances = [];

    renderApp();

    await openDailyDocument();
    await userEvent.keyboard('{Control>}k{/Control}');
    await userEvent.click(await screen.findByText('Delete current document'));

    await waitFor(() => expect(screen.getByText('No document open')).toBeInTheDocument());
    const deleteCall = fetch.mock.calls.find(
      ([input, init]) =>
        String(input) === '/v1/libraries/provenance-lib/documents/daily.md' &&
        init?.method === 'DELETE'
    );
    expect(deleteCall?.[1]?.headers).toMatchObject({
      'X-Quarry-Origin-Id': 'browser:00000000-0000-4000-8000-000000000001',
    });

    act(() => {
      MockEventSource.instances[0].emit('doc.deleted', {
        type: 'doc.deleted',
        library: 'provenance-lib',
        path: 'daily.md',
        doc_id: 'doc-daily',
        origin_id: 'browser:00000000-0000-4000-8000-000000000001',
      });
    });

    expect(screen.queryByText('Deleted externally')).not.toBeInTheDocument();
  });

  it('shows the deleted banner for an external delete of the selected live document', async () => {
    stubBrowserOrigin('00000000-0000-4000-8000-000000000002');
    vi.stubGlobal('fetch', vi.fn(provenanceFetch()));
    vi.stubGlobal('EventSource', MockEventSource);
    MockEventSource.instances = [];

    renderApp();

    await openDailyDocument();
    act(() => {
      MockEventSource.instances[0].emit('doc.deleted', {
        type: 'doc.deleted',
        library: 'provenance-lib',
        path: 'daily.md',
        doc_id: 'doc-daily',
      });
    });

    expect(await screen.findByText('Deleted externally')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Resurrect' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Discard' })).toBeInTheDocument();
  });

  it('clears the deleted banner after a successful resurrect stamped with the browser origin', async () => {
    stubBrowserOrigin('00000000-0000-4000-8000-000000000003');
    const fetch = vi.fn(provenanceFetch());
    vi.stubGlobal('fetch', fetch);
    vi.stubGlobal('EventSource', MockEventSource);
    MockEventSource.instances = [];

    renderApp();

    await showExternalDeleteBanner();
    await userEvent.click(screen.getByRole('button', { name: 'Resurrect' }));

    await waitFor(() => expect(screen.queryByText('Deleted externally')).not.toBeInTheDocument());
    const resurrectCall = fetch.mock.calls.find(
      ([input, init]) =>
        String(input) === '/v1/libraries/provenance-lib/documents/daily.md' &&
        init?.method === 'PUT' &&
        (init.headers as Record<string, string>)['If-None-Match'] === '*'
    );
    expect(resurrectCall?.[1]?.headers).toMatchObject({
      'X-Quarry-Origin-Id': 'browser:00000000-0000-4000-8000-000000000003',
    });
  });

  it('accepts a resurrect 412 when the live document already has the same content', async () => {
    stubBrowserOrigin('00000000-0000-4000-8000-000000000004');
    const fetch = vi.fn(
      provenanceFetch({
        putResponse: () =>
          json({ error: 'already exists' }, { 'content-type': 'application/json' }, 412),
        documentAfterPut: { content: '# Local', etag: '"v2"' },
      })
    );
    vi.stubGlobal('fetch', fetch);
    vi.stubGlobal('EventSource', MockEventSource);
    MockEventSource.instances = [];

    renderApp();

    await showExternalDeleteBanner();
    await userEvent.click(screen.getByRole('button', { name: 'Resurrect' }));

    await waitFor(() => expect(screen.queryByText('Deleted externally')).not.toBeInTheDocument());
    expect(screen.queryByRole('button', { name: 'Review' })).not.toBeInTheDocument();
  });

  it('switches a resurrect 412 with different content to the external-change review path', async () => {
    stubBrowserOrigin('00000000-0000-4000-8000-000000000005');
    vi.stubGlobal(
      'fetch',
      vi.fn(
        provenanceFetch({
          putResponse: () =>
            json({ error: 'already exists' }, { 'content-type': 'application/json' }, 412),
          documentAfterPut: { content: '# Remote', etag: '"v2"' },
        })
      )
    );
    vi.stubGlobal('EventSource', MockEventSource);
    MockEventSource.instances = [];

    renderApp();

    await showExternalDeleteBanner();
    await userEvent.click(screen.getByRole('button', { name: 'Resurrect' }));

    await waitFor(() => expect(screen.queryByText('Deleted externally')).not.toBeInTheDocument());
    expect(screen.getByText(/External version available/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Review' })).toBeInTheDocument();
  });

  it('clears a stale deleted banner when SWR loads an existing selected document', async () => {
    stubBrowserOrigin('00000000-0000-4000-8000-000000000006');
    const documentState = { current: { content: '# Local', etag: '"v1"' } };
    vi.stubGlobal('fetch', vi.fn(provenanceFetch({ documentState })));
    vi.stubGlobal('EventSource', MockEventSource);
    MockEventSource.instances = [];

    renderApp();

    await showExternalDeleteBanner();
    documentState.current = { content: '# Local', etag: '"v2"' };
    act(() => {
      MockEventSource.instances[0].emit('stream.lagged', {
        type: 'stream.lagged',
        library: 'provenance-lib',
      });
    });

    await waitFor(() => expect(screen.queryByText('Deleted externally')).not.toBeInTheDocument());
  });

  it('keeps the selected editor mounted when a collab flush ack outruns the SWR document cache', async () => {
    stubBrowserOrigin('00000000-0000-4000-8000-000000000007');
    vi.stubGlobal('fetch', vi.fn(provenanceFetch()));
    vi.stubGlobal('EventSource', MockEventSource);
    MockEventSource.instances = [];

    renderApp({ mutate: vi.fn(async () => undefined) });

    await openDailyDocument();
    const editor = screen.getByLabelText('Plate markdown editor');

    act(() => {
      testWindow().__quarryTestCollab?.onFlushAck?.({
        etag: '"v2"',
        sessionId: 'browser:peer',
        versionId: 'v2',
      });
    });

    expect(screen.queryByLabelText('Document loading')).not.toBeInTheDocument();
    expect(screen.getByLabelText('Plate markdown editor')).toBe(editor);
  });
});

async function openDailyDocument() {
  await userEvent.click(await screen.findByRole('treeitem', { name: /Daily/ }));
  expect(await screen.findByLabelText('Plate markdown editor')).toHaveValue('# Local');
}

async function showExternalDeleteBanner() {
  await openDailyDocument();
  act(() => {
    MockEventSource.instances[0].emit('doc.deleted', {
      type: 'doc.deleted',
      library: 'provenance-lib',
      path: 'daily.md',
      doc_id: 'doc-daily',
      origin_id: 'external:cli',
    });
  });
  expect(await screen.findByText('Deleted externally')).toBeInTheDocument();
}

function renderApp(config: Record<string, unknown> = {}) {
  const swrConfig = { provider: () => new Map(), ...config } as SWRConfiguration;
  return render(
    <SWRConfig value={swrConfig}>
      <App />
    </SWRConfig>
  );
}

function stubBrowserOrigin(uuid: `${string}-${string}-${string}-${string}-${string}`) {
  vi.spyOn(crypto, 'randomUUID').mockReturnValue(uuid);
}

function provenanceFetch({
  documentState,
  documentAfterPut,
  putResponse,
}: {
  documentState?: { current: { content: string; etag: string } };
  documentAfterPut?: { content: string; etag: string };
  putResponse?: () => Response;
} = {}) {
  let document = { content: '# Local', etag: '"v1"' };
  let useDocumentAfterPut = false;

  return async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url === '/v1/libraries') {
      return json([{ id: 'lib-provenance', slug: 'provenance-lib', created_at: 'now', settings: {} }]);
    }
    if (url === '/v1/libraries/provenance-lib/documents') {
      const current = documentState?.current ?? document;
      return json([
        {
          id: 'doc-daily',
          path: 'daily.md',
          head_version_id: current.etag === '"v1"' ? 'v1' : 'v2',
          content_type: 'text/markdown',
          byte_size: current.content.length,
          metadata: { title: 'Daily' },
          updated_at: 'now',
        },
      ]);
    }
    if (url === '/v1/libraries/provenance-lib/documents/daily.md' && init?.method === 'PUT') {
      if (putResponse) {
        useDocumentAfterPut = true;
        return putResponse();
      }
      document = { content: String(init.body), etag: '"v2"' };
      return json(
        { document: { id: 'doc-daily' }, version: { id: 'v2' } },
        { ETag: '"v2"' }
      );
    }
    if (url === '/v1/libraries/provenance-lib/documents/daily.md' && init?.method === 'DELETE') {
      return json({ id: 'tx-delete' });
    }
    if (url === '/v1/libraries/provenance-lib/documents/daily.md') {
      const current =
        useDocumentAfterPut && documentAfterPut ? documentAfterPut : documentState?.current ?? document;
      return new Response(current.content, {
        headers: {
          ETag: current.etag,
          'content-type': 'text/markdown',
          'x-quarry-document-id': 'doc-daily',
        },
      });
    }
    if (url === '/v1/libraries/provenance-lib/documents/daily.md/presence') {
      return json({ presence: [] });
    }
    if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
      return json({ path: 'daily.md', links: [] });
    }
    if (url.endsWith('/versions')) return json([]);
    if (url === '/v1/libraries/provenance-lib/conflicts') return json([]);
    if (url === '/v1/libraries/provenance-lib/git/peers') return json([]);
    if (url.startsWith('/v1/libraries/provenance-lib/search')) {
      return json({ results: [], cursor: null });
    }
    return new Response('not found', { status: 404 });
  };
}

function json(body: unknown, headers: Record<string, string> = {}, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json', ...headers },
  });
}

class MockEventSource {
  static instances: MockEventSource[] = [];
  readonly listeners = new Map<string, Array<(event: MessageEvent) => void>>();
  onopen: ((event: Event) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;

  constructor(public readonly url: string) {
    MockEventSource.instances.push(this);
    queueMicrotask(() => this.onopen?.(new Event('open')));
  }

  addEventListener(type: string, listener: (event: MessageEvent) => void) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }

  removeEventListener(type: string, listener: (event: MessageEvent) => void) {
    this.listeners.set(
      type,
      (this.listeners.get(type) ?? []).filter((existing) => existing !== listener)
    );
  }

  close() {}

  emit(type: string, payload: Record<string, unknown>) {
    for (const listener of this.listeners.get(type) ?? []) {
      listener(new MessageEvent(type, { data: JSON.stringify(payload) }));
    }
  }
}
