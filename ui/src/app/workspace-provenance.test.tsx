import { MockWebSocket } from '../lib/mock-websocket';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { SWRConfig, type SWRConfiguration } from 'swr';

import type { DocumentEditorConfig } from '../features/editor/MarkdownEditor';
import { App } from './App';

type TestWindow = Window & {
  __quarryTestDocument?: DocumentEditorConfig;
};

function testWindow() {
  return window as TestWindow;
}

vi.mock('../features/editor/MarkdownEditor', () => ({
  MarkdownEditor({ document }: { document: DocumentEditorConfig }) {
    testWindow().__quarryTestDocument = document;
    return <textarea aria-label="Document configuration" readOnly value={document.documentId} />;
  },
}));

describe('workspace document mutation provenance', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    localStorage.clear();
    delete testWindow().__quarryTestDocument;
    window.history.pushState({}, '', '/');
  });

  it('stamps toolbar deletes with the browser origin and clears the selection', async () => {
    stubBrowserOrigin('00000000-0000-4000-8000-000000000001');
    const fetch = vi.fn(provenanceFetch());
    vi.stubGlobal('fetch', fetch);
    vi.stubGlobal('WebSocket', MockWebSocket);
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    MockWebSocket.instances = [];

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
  });

  it('clears the selection when the open document is deleted externally', async () => {
    stubBrowserOrigin('00000000-0000-4000-8000-000000000002');
    vi.stubGlobal('fetch', vi.fn(provenanceFetch()));
    vi.stubGlobal('WebSocket', MockWebSocket);
    MockWebSocket.instances = [];

    renderApp();

    await openDailyDocument();
    act(() => {
      MockWebSocket.instances[0].emit('doc.deleted', {
        type: 'doc.deleted',
        library: 'provenance-lib',
        path: 'daily.md',
        doc_id: 'doc-daily',
        origin_id: 'external:cli',
      });
    });

    // No draft exists to protect or resurrect: deletion simply closes the
    // document (the legacy "Deleted externally" banner died with drafts).
    await waitFor(() => expect(screen.getByText('No document open')).toBeInTheDocument());
  });

  it('wires the session-backed editor to the header save state', async () => {
    stubBrowserOrigin('00000000-0000-4000-8000-000000000003');
    vi.stubGlobal('fetch', vi.fn(provenanceFetch()));
    vi.stubGlobal('WebSocket', MockWebSocket);
    MockWebSocket.instances = [];

    renderApp();

    await openDailyDocument();
    const document = testWindow().__quarryTestDocument;
    expect(document?.documentId).toBe('doc-daily');
    expect(document?.sessionId).toMatch(/^browser:/);

    act(() => document?.onSaveStateChange?.('saving'));
    expect(screen.getByLabelText('Save status')).toHaveTextContent('Saving…');

    act(() => document?.onSaveStateChange?.('reconnecting'));
    expect(screen.getByLabelText('Save status')).toHaveTextContent('Reconnecting');

    act(() => document?.onSaveStateChange?.('refused'));
    expect(screen.getByLabelText('Save status')).toHaveTextContent('Editing unavailable');

    act(() => document?.onSaveStateChange?.('saved'));
    expect(screen.getByLabelText('Save status')).toHaveTextContent('Saved');
  });

  it('wires tmp Markdown collaboration with agent presence', async () => {
    stubBrowserOrigin('00000000-0000-4000-8000-000000000004');
    const secret = '72cb58585aa73e35758bc1141f79e32e';
    window.history.pushState({}, '', `/tmp/${secret}`);
    const fetch = vi.fn(tmpCollabFetch(secret));
    vi.stubGlobal('fetch', fetch);

    renderApp();

    expect(await screen.findByLabelText('Document configuration')).toHaveValue('tmp-doc');
    const document = testWindow().__quarryTestDocument;
    expect(document?.documentId).toBe('tmp-doc');
    expect(document?.sessionId).toBe('browser:00000000-0000-4000-8000-000000000004');

    act(() => document?.onSaveStateChange?.('saving'));
    expect(screen.getByLabelText('Save status')).toHaveTextContent('Saving…');

    act(() => document?.onSaveStateChange?.('saved'));
    expect(screen.getByLabelText('Save status')).toHaveTextContent('Saved');
    expect(screen.getByRole('button', { name: 'Codex · waiting' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: ['Han', 'doff to Agent'].join('') })).not.toBeInTheDocument();

    const removedTmpSignalRoute = ['han', 'doff'].join('');
    expect(fetch.mock.calls.some(([input]) => String(input).includes(`/${removedTmpSignalRoute}`))).toBe(false);
    expect(fetch).not.toHaveBeenCalledWith('/v1/tmp/documents', undefined);
  });

  it('keeps the selected editor mounted across a document version change', async () => {
    stubBrowserOrigin('00000000-0000-4000-8000-000000000007');
    vi.stubGlobal('fetch', vi.fn(provenanceFetch()));
    vi.stubGlobal('WebSocket', MockWebSocket);
    MockWebSocket.instances = [];

    renderApp({ mutate: vi.fn(async () => undefined) });

    await openDailyDocument();
    const editor = screen.getByLabelText('Document configuration');

    act(() => {
      MockWebSocket.instances[0].emit('doc.changed', {
        type: 'doc.changed',
        library: 'provenance-lib',
        path: 'daily.md',
        doc_id: 'doc-daily',
        origin_id: 'agent-injected:document-command:1',
        version_id: 'v2',
      });
    });

    expect(screen.queryByLabelText('Document loading')).not.toBeInTheDocument();
    expect(screen.getByLabelText('Document configuration')).toBe(editor);
  });
});

async function openDailyDocument() {
  await userEvent.click(await screen.findByRole('treeitem', { name: /Daily/ }));
  expect(await screen.findByLabelText('Document configuration')).toHaveValue('doc-daily');
}

function renderApp(config: Record<string, unknown> = {}) {
  localStorage.setItem('quarry:author', 'Tester');
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

function provenanceFetch() {
  let document = { content: '# Local', etag: '"v1"' };

  return async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url === '/v1/libraries') {
      return json([{ id: 'lib-provenance', slug: 'provenance-lib', created_at: 'now', settings: {} }]);
    }
    if (url === '/v1/libraries/provenance-lib/documents') {
      return json([
        {
          id: 'doc-daily',
          path: 'daily.md',
          head_version_id: document.etag === '"v1"' ? 'v1' : 'v2',
          content_type: 'text/markdown',
          byte_size: document.content.length,
          metadata: { title: 'Daily' },
          updated_at: 'now',
        },
      ]);
    }
    if (url === '/v1/libraries/provenance-lib/documents/daily.md' && init?.method === 'PUT') {
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
      return new Response(document.content, {
        headers: {
          ETag: document.etag,
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
    if (url.endsWith('/review')) {
      return json({
        documentId: 'doc-daily',
        baseToken: 'v1',
        comments: [],
        suggestions: [],
        conflicts: [],
      });
    }
    if (url === '/v1/libraries/provenance-lib/conflicts') return json([]);
    if (url === '/v1/libraries/provenance-lib/git/peers') return json([]);
    if (url.startsWith('/v1/libraries/provenance-lib/search')) {
      return json({ results: [], cursor: null });
    }
    return new Response('not found', { status: 404 });
  };
}

function tmpCollabFetch(secret: string) {
  return async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    if (url === '/v1/capabilities') {
      return json({ tmp_documents: true, lib_documents: false });
    }
    if (url === `/v1/tmp/documents/${secret}`) {
      return new Response('# Tmp', {
        headers: {
          ETag: '"tmp-v1"',
          'content-type': 'text/markdown',
          'x-quarry-document-id': 'tmp-doc',
        },
      });
    }
    if (url === `/v1/tmp/documents/${secret}/presence`) {
      return json({
        presence: [
          {
            library: null,
            path: secret,
            documentId: 'tmp-doc',
            agentId: 'ai:codex:tmp',
            status: 'waiting',
            by: 'Codex',
            updatedAt: 'now',
          },
        ],
      });
    }
    if (url === `/v1/tmp/documents/${secret}/review?includeResolved=1`) {
      return json({ documentId: 'tmp-doc', comments: [], suggestions: [], conflicts: [] });
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
