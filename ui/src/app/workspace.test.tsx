import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { SWRConfig } from 'swr';

import { App } from './App';

describe('Quarry Browser workspace', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    localStorage.clear();
    window.history.pushState({}, '', '/');
  });

  it('shows no save status or manual Save button without a live collab session', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-1', slug: 'notes', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/notes/documents') {
        return json([
          {
            id: 'doc-1',
            path: 'daily.md',
            head_version_id: 'v1',
            content_type: 'text/markdown',
            byte_size: 8,
            metadata: { title: 'Daily' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/notes/documents/daily.md' && init?.method !== 'PUT') {
        return new Response('# Daily', { headers: { ETag: '"v1"', 'content-type': 'text/markdown' } });
      }
      if (url === '/v1/libraries/notes/documents/daily.md' && init?.method === 'PUT') {
        expect(init.headers).toMatchObject({ 'If-Match': '"v1"' });
        return json({ version: { id: 'v2' } }, { ETag: '"v2"' });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks') || url.endsWith('/versions')) {
        return json(url.endsWith('/versions') ? [] : { path: 'daily.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/notes/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url === '/v1/libraries/notes/conflicts') {
        return json([]);
      }
      if (url.startsWith('/v1/libraries/notes/search')) {
        return json({ results: [], cursor: null });
      }
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Daily/ }));
    // The save state derives from the live collab session (connection +
    // checkpoint acks); without a session none is shown, there is no manual
    // Save button, and the mode selector is the header's document control.
    // (Save-state round trips need a real websocket, so they live in e2e.)
    expect(await screen.findByRole('button', { name: 'Document mode' })).toHaveTextContent(
      'Editing'
    );
    expect(screen.queryByRole('button', { name: 'Save document' })).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Save status')).not.toBeInTheDocument();
  });

  it('opens Add agent instructions and copies the agent prompt', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    });
    // The prompt text is generated server-side (covered by the Rust
    // agent_prompt tests); this test only verifies the UI mints a token,
    // fetches the prompt with it, and surfaces the server's response.
    const serverPrompt = [
      'Quarry is a local-first collaborative Markdown editor with presence, comments, suggestions, and block edit APIs.',
      '',
      'http://127.0.0.1/lib/agent-lib/documents/folder/live.md?token=invite-agent',
      'trusted-localhost',
      'POST http://127.0.0.1/v1/libraries/agent-lib/documents/folder/live.md/presence',
      'Connected in Quarry and ready.',
    ].join('\n');
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (
        url ===
        '/v1/libraries/agent-lib/documents/folder/live.md/agent-prompt?token=invite-agent'
      ) {
        return new Response(serverPrompt, {
          headers: { 'content-type': 'text/plain; charset=utf-8' },
        });
      }
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-agent', slug: 'agent-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/agent-lib/documents') {
        return json([
          {
            id: 'doc-agent',
            path: 'folder/live.md',
            head_version_id: 'v-agent',
            content_type: 'text/markdown',
            byte_size: 12,
            metadata: { title: 'Live' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/agent-lib/documents/folder/live.md') {
        return new Response('# Live', { headers: { ETag: '"v-agent"', 'content-type': 'text/markdown' } });
      }
      if (url === '/v1/libraries/agent-lib/documents/folder/live.md/share' && init?.method === 'POST') {
        expect(JSON.parse(String(init.body))).toMatchObject({ byHint: 'Tester', role: 'editor' });
        return json({
          id: 'invite-agent',
          document_id: 'doc-agent',
          role: 'editor',
          by_hint: 'Tester',
          created_at: 'now',
          revoked_at: null,
        });
      }
      if (url === '/v1/libraries/agent-lib/documents/folder/live.md/presence') {
        return json({ presence: [] });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'folder/live.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/agent-lib/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url === '/v1/libraries/agent-lib/conflicts') return json([]);
      if (url === '/v1/libraries/agent-lib/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/agent-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Live/ }));
    await userEvent.click(await screen.findByRole('button', { name: 'Add agent' }));

    const dialog = await screen.findByRole('dialog', { name: 'Add agent' });
    await within(dialog).findByText(/Quarry is a local-first collaborative Markdown editor/);
    await within(dialog).findByText(/trusted-localhost/);

    await userEvent.click(within(dialog).getByRole('button', { name: 'Copy instructions' }));
    const copied = writeText.mock.lastCall?.[0] as string;
    expect(copied).toContain('http://127.0.0.1/lib/agent-lib/documents/folder/live.md?token=invite-agent');
    expect(copied).toContain('POST http://127.0.0.1/v1/libraries/agent-lib/documents/folder/live.md/presence');
    expect(copied).toContain('Connected in Quarry and ready.');
    expect(
      await within(dialog).findByRole('button', { name: 'Waiting for your agent…' })
    ).toBeDisabled();
  });

  it('renders agent presence in the document toolbar', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-presence-ui', slug: 'presence-ui', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/presence-ui/documents') {
        return json([
          {
            id: 'doc-presence-ui',
            path: 'live.md',
            head_version_id: 'v-presence',
            content_type: 'text/markdown',
            byte_size: 8,
            metadata: { title: 'Live' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/presence-ui/documents/live.md') {
        return new Response('# Live', { headers: { ETag: '"v-presence"', 'content-type': 'text/markdown' } });
      }
      if (url === '/v1/libraries/presence-ui/documents/live.md/presence') {
        return json({
          presence: [
            {
              library: 'presence-ui',
              path: 'live.md',
              documentId: 'doc-presence-ui',
              agentId: 'ai:codex:abc',
              status: 'waiting',
              by: 'Codex',
              updatedAt: '2026-06-04T12:00:00Z',
            },
          ],
        });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'live.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/presence-ui/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url === '/v1/libraries/presence-ui/conflicts') return json([]);
      if (url === '/v1/libraries/presence-ui/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/presence-ui/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Live/ }));
    expect(await screen.findByLabelText('Codex · waiting')).toBeInTheDocument();
  });

  it('persists the selected right pane tab per library', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-tabs', slug: 'right-tabs', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/right-tabs/documents') {
        return json([
          {
            id: 'doc-tabs',
            path: 'notes/tabbed.md',
            head_version_id: 'v-tabs',
            content_type: 'text/markdown',
            byte_size: 12,
            metadata: { title: 'Tabbed' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/right-tabs/documents/notes/tabbed.md') {
        return new Response('# Tabbed', { headers: { ETag: '"v-tabs"', 'content-type': 'text/markdown' } });
      }
      if (url.endsWith('/outgoing-links')) {
        return json({
          path: 'notes/tabbed.md',
          links: [link({ target_kind: 'wiki_link', target_text: 'Guide', target_path: 'guide.md', resolved: true })],
        });
      }
      if (url.endsWith('/backlinks')) {
        return json({
          path: 'notes/tabbed.md',
          links: [link({ src_path: 'source.md', target_kind: 'wiki_link', target_text: 'Tabbed', resolved: true })],
        });
      }
      if (url.startsWith('/v1/libraries/right-tabs/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url === '/v1/libraries/right-tabs/conflicts') return json([]);
      if (url.startsWith('/v1/libraries/right-tabs/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    const { unmount } = renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Tabbed/ }));
    expect(screen.getByRole('tab', { name: 'Links' })).toHaveAttribute('aria-selected', 'true');
    // The Links tab combines outgoing links and backlinks in one panel.
    expect(await screen.findByText('guide.md')).toBeInTheDocument();
    expect(await screen.findByText('source.md')).toBeInTheDocument();

    await userEvent.click(screen.getByRole('tab', { name: 'Versions' }));
    expect(screen.getByRole('tab', { name: 'Versions' })).toHaveAttribute('aria-selected', 'true');
    expect(localStorage.getItem('quarry:right-pane-tab:right-tabs')).toBe('versions');

    unmount();
    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Tabbed/ }));
    expect(screen.getByRole('tab', { name: 'Versions' })).toHaveAttribute('aria-selected', 'true');
  });

  it('loads the selected library and document from the route and updates the route on open', async () => {
    window.history.pushState({}, '', '/lib/routed-lib/documents/folder/deep.md');
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-route', slug: 'routed-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/routed-lib/documents') {
        return json([
          {
            id: 'doc-deep',
            path: 'folder/deep.md',
            head_version_id: 'v-deep',
            content_type: 'text/markdown',
            byte_size: 6,
            metadata: { title: 'Deep' },
            updated_at: 'now',
          },
          {
            id: 'doc-next',
            path: 'next.md',
            head_version_id: 'v-next',
            content_type: 'text/markdown',
            byte_size: 6,
            metadata: { title: 'Next' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/routed-lib/documents/folder/deep.md') {
        return new Response('# Deep', { headers: { ETag: '"v-deep"', 'content-type': 'text/markdown' } });
      }
      if (url === '/v1/libraries/routed-lib/documents/next.md') {
        return new Response('# Next', { headers: { ETag: '"v-next"', 'content-type': 'text/markdown' } });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'document.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/routed-lib/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url === '/v1/libraries/routed-lib/conflicts') return json([]);
      if (url === '/v1/libraries/routed-lib/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/routed-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await waitFor(() => expect(screen.getByLabelText('Plate markdown editor')).toHaveTextContent('Deep'));
    await userEvent.click(screen.getByRole('treeitem', { name: /Next/ }));

    await waitFor(() => expect(screen.getByLabelText('Plate markdown editor')).toHaveTextContent('Next'));
    expect(window.location.pathname).toBe('/lib/routed-lib/documents/next.md');
  });

  it('loads routed tmp Markdown document controls through tmp APIs', async () => {
    const secret = '72cb58585aa73e35758bc1141f79e32e';
    window.history.pushState({}, '', `/tmp/${secret}`);
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-notes', slug: 'notes', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/notes/documents') return json([]);
      if (url === `/v1/tmp/documents/${secret}`) {
        return new Response('# Tmp\n', {
          headers: {
            ETag: '"tmp-v1"',
            'content-type': 'text/markdown',
            'x-quarry-document-id': 'tmp-1',
            'x-quarry-expires-at': '2099-01-01T00:00:00Z',
          },
        });
      }
      if (url === `/v1/tmp/documents/${secret}/presence`) return json({ presence: [] });
      if (url === `/v1/tmp/documents/${secret}/review?includeResolved=1`) {
        return json({ documentId: 'tmp-1', comments: [], suggestions: [], conflicts: [] });
      }
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    expect(await screen.findByRole('button', { name: 'Document mode' })).toHaveTextContent(
      'Editing'
    );
    expect(window.location.pathname).toBe(`/tmp/${secret}`);
    expect(fetch).not.toHaveBeenCalledWith('/v1/tmp/documents', undefined);
    expect(screen.queryByLabelText('Document tree')).not.toBeInTheDocument();
    expect(screen.queryByText(/Expires/)).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Add agent' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();
    fireEvent.pointerDown(screen.getByRole('button', { name: 'Document actions' }));
    expect(await screen.findByText('Download as Markdown')).toBeInTheDocument();
    expect(screen.queryByText('Copy invite link')).not.toBeInTheDocument();
    expect(screen.queryByText(/Extend TTL/)).not.toBeInTheDocument();
  });

  it('loads a routed tmp Markdown document and titles the page from its H1', async () => {
    const secret = '63895bec2fda4380b44a240f8ca57075';
    window.history.pushState({}, '', `/tmp/${secret}`);
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/capabilities') {
        return json({ tmp_documents: true, lib_documents: false });
      }
      if (url === `/v1/tmp/documents/${secret}`) {
        return new Response('# Tmp Workspace\n\nBody text.\n', {
          headers: {
            ETag: '"tmp-v1"',
            'content-type': 'text/markdown',
            'x-quarry-document-id': 'tmp-title',
          },
        });
      }
      if (url === `/v1/tmp/documents/${secret}/presence`) return json({ presence: [] });
      if (url === `/v1/tmp/documents/${secret}/review?includeResolved=1`) {
        return json({ documentId: 'tmp-title', comments: [], suggestions: [], conflicts: [] });
      }
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    expect(await screen.findByRole('button', { name: 'Document mode' })).toHaveTextContent('Editing');
    await waitFor(() => expect(window.document.title).toBe('Tmp Workspace · Quarry'));
    expect(window.location.pathname).toBe(`/tmp/${secret}`);
  });

  it('uses the tmp workspace and hides library controls when library documents are disabled', async () => {
    const secret = '72cb58585aa73e35758bc1141f79e32e';
    window.history.pushState({}, '', `/tmp/${secret}`);
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/capabilities') {
        return json({ tmp_documents: true, lib_documents: false });
      }
      if (url === `/v1/tmp/documents/${secret}`) {
        return new Response('# Tmp\n', {
          headers: {
            ETag: '"tmp-v1"',
            'content-type': 'text/markdown',
            'x-quarry-document-id': 'tmp-1',
            'x-quarry-expires-at': '2099-01-01T00:00:00Z',
          },
        });
      }
      if (url === `/v1/tmp/documents/${secret}/presence`) return json({ presence: [] });
      if (url === `/v1/tmp/documents/${secret}/review?includeResolved=1`) {
        return json({ documentId: 'tmp-1', comments: [], suggestions: [], conflicts: [] });
      }
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    expect(await screen.findByRole('button', { name: 'Document mode' })).toHaveTextContent(
      'Editing'
    );

    expect(window.location.pathname).toBe(`/tmp/${secret}`);
    expect(fetch).not.toHaveBeenCalledWith('/v1/libraries', undefined);
    expect(fetch).not.toHaveBeenCalledWith('/v1/tmp/documents', undefined);
    expect(screen.queryByLabelText('Document tree')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Document details')).not.toBeInTheDocument();
    expect(screen.queryByRole('combobox', { name: 'Library switcher' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Search' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Add agent' })).toBeInTheDocument();
    expect(screen.queryByRole('tab', { name: 'Versions' })).not.toBeInTheDocument();
    expect(screen.queryByText(/Expires/)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();
  });

  it('does not mount a routed editor before the document body loads', async () => {
    window.history.pushState({}, '', '/lib/race-lib/documents/deep.md');
    let resolveDocument: () => void = () => {};
    const documentReady = new Promise<void>((resolve) => {
      resolveDocument = resolve;
    });
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-race', slug: 'race-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/race-lib/documents') {
        return json([
          {
            id: 'doc-deep',
            path: 'deep.md',
            head_version_id: 'v-deep',
            content_type: 'text/markdown',
            byte_size: 12,
            metadata: { title: 'Deep' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/race-lib/documents/deep.md') {
        await documentReady;
        return new Response('# Deep body', {
          headers: { ETag: '"v-deep"', 'content-type': 'text/markdown' },
        });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'deep.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/race-lib/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url === '/v1/libraries/race-lib/conflicts') return json([]);
      if (url === '/v1/libraries/race-lib/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/race-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await screen.findByRole('treeitem', { name: /Deep/ });
    await act(async () => {
      resolveDocument();
      await documentReady;
    });

    await waitFor(() => expect(screen.getByLabelText('Plate markdown editor')).toHaveTextContent('Deep body'));
  });

  it('offers tree context menu actions for folders and documents', async () => {
    const prompt = vi
      .spyOn(window, 'prompt')
      .mockReturnValueOnce('folder/new.md')
      .mockReturnValueOnce('folder/moved.md');
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    const writeText = vi.fn();
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    });
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-tree', slug: 'tree-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/tree-lib/documents') {
        return json([
          {
            id: 'doc-source',
            path: 'folder/source.md',
            head_version_id: 'v-source',
            content_type: 'text/markdown',
            byte_size: 8,
            metadata: { title: 'Source' },
            updated_at: 'now',
          },
          {
            id: 'doc-other',
            path: 'folder/other.md',
            head_version_id: 'v-other',
            content_type: 'text/markdown',
            byte_size: 7,
            metadata: { title: 'Other' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/tree-lib/documents/folder/source.md') {
        return new Response('# Source', { headers: { ETag: '"v-source"', 'content-type': 'text/markdown' } });
      }
      if (url === '/v1/libraries/tree-lib/documents/folder/new.md' && init?.method === 'PUT') {
        expect(init.headers).toMatchObject({ 'If-None-Match': '*' });
        return json({ version: { id: 'v-new' } }, { ETag: '"v-new"' });
      }
      if (url === '/v1/libraries/tree-lib/documents/folder/new.md') {
        return new Response('# Untitled\n', { headers: { ETag: '"v-new"', 'content-type': 'text/markdown' } });
      }
      if (url === '/v1/libraries/tree-lib/documents/folder/source.md/move' && init?.method === 'POST') {
        expect(JSON.parse(String(init.body))).toMatchObject({ to_path: 'folder/moved.md' });
        return json({});
      }
      if (url === '/v1/libraries/tree-lib/documents/folder/other.md' && init?.method === 'DELETE') {
        return json({});
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'document.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/tree-lib/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url === '/v1/libraries/tree-lib/conflicts') return json([]);
      if (url === '/v1/libraries/tree-lib/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/tree-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    const treeItemLabel = (name: RegExp, text: string) =>
      within(screen.getByRole('treeitem', { name })).getByText(text);

    await screen.findByRole('treeitem', { name: /folder/ });
    fireEvent.contextMenu(treeItemLabel(/folder/, 'folder'), { clientX: 32, clientY: 48 });
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Copy path' }));
    expect(writeText).toHaveBeenCalledWith('folder');
    await waitFor(() => expect(screen.queryByRole('menu')).not.toBeInTheDocument());

    fireEvent.contextMenu(treeItemLabel(/folder/, 'folder'), { clientX: 32, clientY: 48 });
    await userEvent.click(await screen.findByRole('menuitem', { name: 'New document here' }));
    expect(prompt).toHaveBeenCalledWith('New document path', 'folder/untitled.md');
    await waitFor(() =>
      expect(fetch).toHaveBeenCalledWith(
        '/v1/libraries/tree-lib/documents/folder/new.md',
        expect.objectContaining({ method: 'PUT' })
      )
    );
    await waitFor(() => expect(screen.queryByRole('menu')).not.toBeInTheDocument());

    await userEvent.click(screen.getByRole('treeitem', { name: /Source/ }));
    await waitFor(() => expect(screen.getByLabelText('Plate markdown editor')).toHaveTextContent('Source'));
    expect(window.location.pathname).toBe('/lib/tree-lib/documents/folder/source.md');

    fireEvent.contextMenu(treeItemLabel(/Source/, 'Source'), { clientX: 40, clientY: 72 });
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Move' }));
    expect(prompt).toHaveBeenCalledWith('Move document to path', 'folder/source.md');
    await waitFor(() =>
      expect(fetch).toHaveBeenCalledWith(
        '/v1/libraries/tree-lib/documents/folder/source.md/move',
        expect.objectContaining({ method: 'POST' })
      )
    );
    await waitFor(() => expect(screen.queryByRole('menu')).not.toBeInTheDocument());

    fireEvent.contextMenu(treeItemLabel(/Other/, 'Other'), { clientX: 40, clientY: 100 });
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Delete' }));
    expect(confirm).toHaveBeenCalledWith('Delete folder/other.md?');
    await waitFor(() =>
      expect(fetch).toHaveBeenCalledWith(
        '/v1/libraries/tree-lib/documents/folder/other.md',
        expect.objectContaining({ method: 'DELETE' })
      )
    );
  });

  it('renames a focused tree document from the keyboard', async () => {
    const prompt = vi.spyOn(window, 'prompt').mockReturnValueOnce('folder/keyed.md');
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-tree-keyboard', slug: 'tree-keyboard', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/tree-keyboard/documents') {
        return json([
          {
            id: 'doc-source',
            path: 'folder/source.md',
            head_version_id: 'v-source',
            content_type: 'text/markdown',
            byte_size: 8,
            metadata: { title: 'Source' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/tree-keyboard/documents/folder/source.md/move' && init?.method === 'POST') {
        expect(JSON.parse(String(init.body))).toMatchObject({ to_path: 'folder/keyed.md' });
        return json({});
      }
      if (url === '/v1/libraries/tree-keyboard/conflicts') return json([]);
      if (url === '/v1/libraries/tree-keyboard/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/tree-keyboard/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    const treeItem = await screen.findByRole('treeitem', { name: /Source/ });
    treeItem.focus();
    fireEvent.keyDown(treeItem, { key: 'F2' });

    expect(prompt).toHaveBeenCalledTimes(1);
    expect(prompt).toHaveBeenCalledWith('Move document to path', 'folder/source.md');
    await waitFor(() =>
      expect(fetch).toHaveBeenCalledWith(
        '/v1/libraries/tree-keyboard/documents/folder/source.md/move',
        expect.objectContaining({ method: 'POST' })
      )
    );
  });

  it('opens tree context actions from the keyboard', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-tree-menu-keyboard', slug: 'tree-menu-keyboard', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/tree-menu-keyboard/documents') {
        return json([
          {
            id: 'doc-source',
            path: 'folder/source.md',
            head_version_id: 'v-source',
            content_type: 'text/markdown',
            byte_size: 8,
            metadata: { title: 'Source' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/tree-menu-keyboard/conflicts') return json([]);
      if (url === '/v1/libraries/tree-menu-keyboard/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/tree-menu-keyboard/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    const treeItem = await screen.findByRole('treeitem', { name: /Source/ });
    treeItem.focus();
    fireEvent.keyDown(treeItem, { key: 'ContextMenu' });

    const menu = await screen.findByRole('menu', { name: 'Actions for folder/source.md' });
    expect(within(menu).getByRole('menuitem', { name: 'Move' })).toBeInTheDocument();
    expect(within(menu).getByRole('menuitem', { name: 'Copy path' })).toBeInTheDocument();
  });

  it('persists tree expansion state per library', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-tree-state', slug: 'tree-state', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/tree-state/documents') {
        return json([
          {
            id: 'doc-a',
            path: 'folder/a.md',
            head_version_id: 'v-a',
            content_type: 'text/markdown',
            byte_size: 3,
            metadata: { title: 'A' },
            updated_at: 'now',
          },
          {
            id: 'doc-b',
            path: 'folder/b.md',
            head_version_id: 'v-b',
            content_type: 'text/markdown',
            byte_size: 3,
            metadata: { title: 'B' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/tree-state/conflicts') return json([]);
      if (url === '/v1/libraries/tree-state/git/peers') return json([]);
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    const { unmount } = renderApp();

    await screen.findByRole('treeitem', { name: /A/ });
    await userEvent.click(screen.getByRole('treeitem', { name: /folder/ }));
    await waitFor(() => expect(screen.queryByRole('treeitem', { name: /A/ })).not.toBeInTheDocument());

    unmount();
    renderApp();

    await screen.findByRole('treeitem', { name: /folder/ });
    expect(screen.queryByRole('treeitem', { name: /A/ })).not.toBeInTheDocument();
    expect(JSON.parse(localStorage.getItem('quarry:tree-open:tree-state') ?? '{}')).toMatchObject({
      'folder:folder': false,
    });
  });

  it('shows resolved and unresolved link state in the side pane', async () => {
    const prompt = vi.spyOn(window, 'prompt').mockReturnValueOnce('missing.md');
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-2', slug: 'link-notes', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/link-notes/documents') {
        return json([
          {
            id: 'doc-3',
            path: 'links.md',
            head_version_id: 'v1',
            content_type: 'text/markdown',
            byte_size: 8,
            metadata: { title: 'Links' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/link-notes/documents/links.md') {
        return new Response('# Links\n\nSee [[Guide]], [[Missing]], and [Guide link](guide.md).', {
          headers: { ETag: '"v1"', 'content-type': 'text/markdown' },
        });
      }
      if (url === '/v1/libraries/link-notes/documents/guide.md') {
        return new Response('# Guide\n\nHover preview body.', {
          headers: { ETag: '"guide"', 'content-type': 'text/markdown' },
        });
      }
      if (url === '/v1/libraries/link-notes/documents/missing.md' && init?.method === 'PUT') {
        expect(init.headers).toMatchObject({ 'If-None-Match': '*' });
        return json({ version: { id: 'v-missing' } }, { ETag: '"missing"' });
      }
      if (url === '/v1/libraries/link-notes/documents/missing.md') {
        return new Response('# Untitled\n', { headers: { ETag: '"missing"', 'content-type': 'text/markdown' } });
      }
      if (url.endsWith('/outgoing-links')) {
        return json({
          path: 'links.md',
          links: [
            link({ target_kind: 'wiki_link', target_text: 'Guide', target_path: 'guide.md', resolved: true }),
            link({ target_kind: 'wiki_link', target_text: 'Missing', target_path: null, start_offset: 12, resolved: false }),
            link({
              target_kind: 'wiki_link',
              target_text: 'Duplicate',
              target_path: null,
              start_offset: 20,
              resolution_status: 'ambiguous',
              resolved: false,
            }),
            link({ target_kind: 'markdown_link', target_text: 'guide.md', target_path: 'guide.md', resolved: true }),
            link({ target_kind: 'heading', target_text: 'Links', target_path: 'links.md', start_offset: 24, resolved: true }),
            link({
              target_kind: 'markdown_link',
              target_text: 'https://example.com',
              target_path: null,
              start_offset: 30,
              resolution_status: 'external',
              resolved: false,
            }),
          ],
        });
      }
      if (url.endsWith('/backlinks')) {
        return json({
          path: 'links.md',
          links: [link({ src_path: 'source.md', target_kind: 'wiki_link', target_text: 'Links', resolved: true })],
        });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url.startsWith('/v1/libraries/link-notes/graph')) {
        return json({
          nodes: [
            { id: 'doc-3', path: 'links.md', title: 'Links', content_type: 'text/markdown' },
            { id: 'doc-2', path: 'guide.md', title: 'Guide', content_type: 'text/markdown' },
          ],
          edges: [
            {
              id: 'edge-1',
              source: 'doc-3',
              source_path: 'links.md',
              target: 'doc-2',
              target_path: 'guide.md',
              target_kind: 'wiki_link',
              target_text: 'Guide',
              resolved: true,
            },
          ],
          truncated: false,
        });
      }
      if (url === '/v1/libraries/link-notes/conflicts') return json([]);
      if (url.startsWith('/v1/libraries/link-notes/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Links/ }));

    // Scope link assertions to the details pane: the editor now also renders
    // `[[Guide]]`/`[[Missing]]` as wiki-link chips with the same text.
    await screen.findByLabelText('Plate markdown editor');
    const details = within(screen.getByLabelText('Document details'));
    expect(await details.findAllByText('guide.md')).toHaveLength(2);
    expect(details.getByText('Missing')).toBeInTheDocument();
    expect(details.getByText('Unresolved')).toBeInTheDocument();
    expect(details.getByText('Duplicate')).toBeInTheDocument();
    expect(details.getByText('Ambiguous')).toBeInTheDocument();
    expect(details.queryByRole('button', { name: 'Create document for Duplicate' })).not.toBeInTheDocument();
    // The Links panel only lists library-document references: headings and external
    // URLs have no document destination and are filtered out.
    expect(details.queryByText('# Links')).not.toBeInTheDocument();
    expect(details.queryByText('https://example.com')).not.toBeInTheDocument();
    expect(screen.getByLabelText('Plate markdown editor')).toHaveTextContent('Links');
    const resolvedLinkButtons = details.getAllByRole('button', { name: 'guide.md' });
    await userEvent.hover(resolvedLinkButtons[0]);
    expect((await screen.findAllByLabelText('Link preview'))[0]).toHaveTextContent('Hover preview body.');
    await userEvent.unhover(resolvedLinkButtons[0]);
    await waitFor(() => expect(screen.queryAllByLabelText('Link preview')).toHaveLength(0));
    // Backlinks render in the same Links panel as outgoing links.
    expect(details.getByText('source.md')).toBeInTheDocument();
    await userEvent.click(details.getByRole('button', { name: 'Create document for Missing' }));
    expect(prompt).toHaveBeenCalledWith('New document path', 'Missing.md');
    await waitFor(() => expect(screen.getByLabelText('Plate markdown editor')).toHaveTextContent('Untitled'));

    await userEvent.click(details.getAllByRole('button', { name: 'guide.md' })[0]);
    await waitFor(() =>
      expect(screen.getByLabelText('Plate markdown editor')).toHaveTextContent('Hover preview body.')
    );
  });

  it('opens a historical version diff and restores the selected version', async () => {
    let restored = false;
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-3', slug: 'versions-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/versions-lib/documents') {
        return json([
          {
            id: 'doc-versions',
            path: 'versioned.md',
            head_version_id: restored ? 'v3' : 'v2',
            content_type: 'text/markdown',
            byte_size: 10,
            metadata: { title: 'Versioned' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/versions-lib/documents/versioned.md') {
        return new Response(restored ? '# Old' : '# Current', {
          headers: { ETag: restored ? '"v3"' : '"v2"', 'content-type': 'text/markdown' },
        });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'versioned.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/versions-lib/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) {
        return json([historyEntry(restored ? 'v3' : 'v2'), historyEntry('v1')]);
      }
      if (url.endsWith('/versions/v1')) {
        return json({ version: version('v1'), content: '# Old' });
      }
      if (url.includes('/versions/v1/diff')) {
        return json({
          base_version_id: 'v1',
          against_version_id: restored ? 'v3' : 'v2',
          unified_diff: '--- base\n+++ against\n-# Old\n+# Current\n',
        });
      }
      if (url.endsWith('/versions/v1/restore') && init?.method === 'POST') {
        restored = true;
        return json({ outcome: { version: version('v3') } }, { ETag: '"v3"' });
      }
      if (url === '/v1/libraries/versions-lib/conflicts') return json([]);
      if (url.startsWith('/v1/libraries/versions-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Versioned/ }));
    await userEvent.click(screen.getByRole('tab', { name: 'Versions' }));
    await userEvent.click(await screen.findByLabelText('View version v1'));

    expect(await screen.findByText('# Old')).toBeInTheDocument();
    expect(screen.getByText(/-# Old/)).toBeInTheDocument();

    await userEvent.click(screen.getByLabelText('Restore version v1'));
    await waitFor(() => expect(screen.getByLabelText('Plate markdown editor')).toHaveTextContent('Old'));
    expect(fetch).toHaveBeenCalledWith(
      '/v1/libraries/versions-lib/documents/versioned.md/versions/v1/restore',
      expect.objectContaining({ method: 'POST' })
    );
  });

  it('shows version metadata in the version history list', async () => {
    const historicalVersion = {
      id: 'v-meta',
      document_id: 'doc-version-meta',
      latest_version_id: 'v-meta',
      earliest_version_id: 'v-meta',
      raw_version_count: 1,
      actor: 'Avery',
      source: 'git',
      message: 'Imported from Git',
      provenance: { remote: 'origin/main' },
      checkpoint_reason: null,
      content_type: 'text/markdown',
      byte_size: 2048,
      created_at: '2026-05-28T12:00:00Z',
      updated_at: '2026-05-28T12:00:00Z',
    };
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-version-meta', slug: 'version-meta-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/version-meta-lib/documents') {
        return json([
          {
            id: 'doc-version-meta',
            path: 'meta.md',
            head_version_id: 'v-meta',
            content_type: 'text/markdown',
            byte_size: 2048,
            metadata: { title: 'Meta' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/version-meta-lib/documents/meta.md') {
        return new Response('# Meta', {
          headers: { ETag: '"v-meta"', 'content-type': 'text/markdown' },
        });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'meta.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/version-meta-lib/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) return json([historicalVersion]);
      if (url === '/v1/libraries/version-meta-lib/conflicts') return json([]);
      if (url.startsWith('/v1/libraries/version-meta-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Meta/ }));
    await userEvent.click(screen.getByRole('tab', { name: 'Versions' }));

    const transactionText = await screen.findByText(/Source git/);
    const versionRow = transactionText.closest('li');
    if (!versionRow) throw new Error('Expected version metadata to render inside a version row');
    expect(versionRow).toHaveTextContent('v-meta');
    expect(versionRow).toHaveTextContent('2026-05-28T12:00:00Z');
    expect(versionRow).toHaveTextContent('text/markdown');
    expect(versionRow).toHaveTextContent('2 KB');
    expect(versionRow).toHaveTextContent('Imported from Git');
    expect(versionRow).toHaveTextContent('git');
    expect(versionRow).toHaveTextContent('Avery');
    expect(versionRow).toHaveTextContent('origin/main');
  });

  it('diffs two selected historical versions', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-version-compare', slug: 'version-compare-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/version-compare-lib/documents') {
        return json([
          {
            id: 'doc-version-compare',
            path: 'compare.md',
            head_version_id: 'v3',
            content_type: 'text/markdown',
            byte_size: 7,
            metadata: { title: 'Compare' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/version-compare-lib/documents/compare.md') {
        return new Response('# Three', {
          headers: { ETag: '"v3"', 'content-type': 'text/markdown' },
        });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'compare.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/version-compare-lib/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) return json([historyEntry('v3'), historyEntry('v2'), historyEntry('v1')]);
      if (url.endsWith('/versions/v1')) {
        return json({ version: version('v1'), content: '# One' });
      }
      if (url.includes('/versions/v1/diff')) {
        const against = new URL(url, 'http://quarry.local').searchParams.get('against');
        return json({
          base_version_id: 'v1',
          against_version_id: against ?? 'v3',
          unified_diff:
            against === 'v2' ? '--- base\n+++ against\n-# One\n+# Two\n' : '--- base\n+++ against\n-# One\n+# Three\n',
        });
      }
      if (url === '/v1/libraries/version-compare-lib/conflicts') return json([]);
      if (url.startsWith('/v1/libraries/version-compare-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Compare/ }));
    await userEvent.click(screen.getByRole('tab', { name: 'Versions' }));
    await userEvent.click(await screen.findByLabelText('View version v1'));
    await userEvent.selectOptions(await screen.findByLabelText('Compare version against'), 'v2');

    expect(await screen.findByText(/\+# Two/)).toBeInTheDocument();
    expect(fetch).toHaveBeenCalledWith(
      '/v1/libraries/version-compare-lib/documents/compare.md/versions/v1/diff?against=v2',
      undefined
    );
  });

  it('refreshes the open document and indexed link state from event stream updates', async () => {
    let content = '# Initial';
    let outgoing = [] as ReturnType<typeof link>[];
    let openConflicts = [] as ReturnType<typeof conflict>[];
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-4', slug: 'events-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/events-lib/documents') {
        return json([
          {
            id: 'doc-events',
            path: 'daily.md',
            head_version_id: content === '# Initial' ? 'v1' : 'v2',
            content_type: 'text/markdown',
            byte_size: content.length,
            metadata: { title: 'Daily' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/events-lib/documents/daily.md') {
        return new Response(content, {
          headers: { ETag: content === '# Initial' ? '"v1"' : '"v2"', 'content-type': 'text/markdown' },
        });
      }
      if (url.endsWith('/outgoing-links')) {
        return json({ path: 'daily.md', links: outgoing });
      }
      if (url.endsWith('/backlinks')) {
        return json({ path: 'daily.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/events-lib/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url === '/v1/libraries/events-lib/conflicts') return json(openConflicts);
      if (url.startsWith('/v1/libraries/events-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);
    vi.stubGlobal('EventSource', MockEventSource);
    MockEventSource.instances = [];

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Daily/ }));
    expect(await screen.findByLabelText('Plate markdown editor')).toHaveTextContent('Initial');
    expect(MockEventSource.instances[0]?.url).toBe('/v1/events?library=events-lib');

    content = '# External';
    act(() => {
      MockEventSource.instances[0].emit('doc.changed', {
        type: 'doc.changed',
        library: 'events-lib',
        path: 'daily.md',
      });
    });

    await waitFor(() => expect(screen.getByLabelText('Plate markdown editor')).toHaveTextContent('External'));

    outgoing = [link({ src_path: 'daily.md', target_text: 'Guide', target_path: 'guide.md' })];
    act(() => {
      MockEventSource.instances[0].emit('library.reindexed', {
        type: 'library.reindexed',
        library: 'events-lib',
      });
    });

    await waitFor(() => expect(screen.getByText('guide.md')).toBeInTheDocument());

    content = '# Git synced';
    openConflicts = [conflict('git-conflict')];
    act(() => {
      MockEventSource.instances[0].emit('git.sync.completed', {
        type: 'git.sync.completed',
        library: 'events-lib',
        peer_id: 'peer-main',
        applied: 2,
        conflicts: 1,
      });
    });

    await waitFor(() => expect(screen.getByLabelText('Plate markdown editor')).toHaveTextContent('Git synced'));
  });

  it('seeds recreated same-path documents instead of showing the deleted document cache', async () => {
    let documentExists = true;
    let recreated = false;
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    const prompt = vi.spyOn(window, 'prompt').mockReturnValue('daily.md');
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-cache', slug: 'cache-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/cache-lib/documents') {
        return json(
          documentExists
            ? [
                {
                  id: recreated ? 'doc-new' : 'doc-old',
                  path: 'daily.md',
                  head_version_id: recreated ? 'v-new' : 'v-old',
                  content_type: 'text/markdown',
                  byte_size: recreated ? 11 : 16,
                  metadata: { title: 'Daily' },
                  updated_at: 'now',
                },
              ]
            : []
        );
      }
      if (url === '/v1/libraries/cache-lib/documents/daily.md' && init?.method === 'DELETE') {
        documentExists = false;
        return json({ id: 'tx-delete' });
      }
      if (url === '/v1/libraries/cache-lib/documents/daily.md' && init?.method === 'PUT') {
        documentExists = true;
        recreated = true;
        return json(writeOutcome('', 'v-new', 'daily.md'), { ETag: '"v-new"' });
      }
      if (url === '/v1/libraries/cache-lib/documents/daily.md') {
        if (recreated) return new Promise<Response>(() => {});
        return new Response('Old cached body\n', {
          headers: {
            ETag: '"v-old"',
            'content-type': 'text/markdown',
          },
        });
      }
      if (url === '/v1/libraries/cache-lib/documents/daily.md/presence') {
        return json({ presence: [] });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'daily.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/cache-lib/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) return json(recreated ? [historyEntry('v-new')] : [historyEntry('v-old')]);
      if (url === '/v1/libraries/cache-lib/conflicts') return json([]);
      if (url === '/v1/libraries/cache-lib/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/cache-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);
    vi.stubGlobal('EventSource', MockEventSource);
    MockEventSource.instances = [];
    window.history.pushState({}, '', '/lib/cache-lib/documents/daily.md');

    renderApp();

    await waitFor(() => expect(fetch).toHaveBeenCalledWith('/v1/libraries/cache-lib/documents/daily.md'));
    await waitFor(() => expect(screen.getByLabelText('Plate markdown editor')).toHaveTextContent('Old cached body'));

    await userEvent.keyboard('{Control>}k{/Control}');
    await userEvent.click(await screen.findByText('Delete current document'));
    await waitFor(() => expect(screen.queryByLabelText('Plate markdown editor')).not.toBeInTheDocument());

    await userEvent.click(screen.getByRole('button', { name: 'Create document' }));
    const editor = await screen.findByLabelText('Plate markdown editor');
    expect(editor).toHaveTextContent('Untitled');
    expect(editor).not.toHaveTextContent('Old cached body');
    expect(confirm).toHaveBeenCalledWith('Delete daily.md?');
    expect(prompt).toHaveBeenCalledWith('New document path', 'untitled.md');
  });

  it('falls back to polling when the event stream errors', async () => {
    let content = '# Initial';
    let outgoing = [] as ReturnType<typeof link>[];
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-polling', slug: 'polling-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/polling-lib/documents') {
        return json([
          {
            id: 'doc-polling',
            path: 'daily.md',
            head_version_id: content === '# Initial' ? 'v1' : 'v2',
            content_type: 'text/markdown',
            byte_size: content.length,
            metadata: { title: 'Daily' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/polling-lib/documents/daily.md') {
        return new Response(content, {
          headers: { ETag: content === '# Initial' ? '"v1"' : '"v2"', 'content-type': 'text/markdown' },
        });
      }
      if (url.endsWith('/outgoing-links')) {
        return json({ path: 'daily.md', links: outgoing });
      }
      if (url.endsWith('/backlinks')) {
        return json({ path: 'daily.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/polling-lib/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url === '/v1/libraries/polling-lib/conflicts') return json([]);
      if (url === '/v1/libraries/polling-lib/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/polling-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);
    vi.stubGlobal('EventSource', MockEventSource);
    MockEventSource.instances = [];

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Daily/ }));
    expect(await screen.findByLabelText('Plate markdown editor')).toHaveTextContent('Initial');

    content = '# Polled';
    outgoing = [link({ src_path: 'daily.md', target_text: 'Guide', target_path: 'guide.md' })];
    act(() => {
      MockEventSource.instances[0].onerror?.(new Event('error'));
    });

    await waitFor(() => expect(screen.getByLabelText('Plate markdown editor')).toHaveTextContent('Polled'));
    expect(screen.getByText('guide.md')).toBeInTheDocument();
  });

  it('marks an open conflict resolved from the conflict panel', async () => {
    let resolved = false;
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-5', slug: 'conflicts-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/conflicts-lib/documents') {
        return json([
          {
            id: 'doc-conflict',
            path: 'conflict.md',
            head_version_id: 'v1',
            content_type: 'text/markdown',
            byte_size: 10,
            metadata: { title: 'Conflict' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/conflicts-lib/documents/conflict.md') {
        return new Response('# Conflict', { headers: { ETag: '"v1"', 'content-type': 'text/markdown' } });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'conflict.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/conflicts-lib/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url === '/v1/libraries/conflicts-lib/conflicts') {
        return json(resolved ? [] : [conflict('conflict-1')]);
      }
      if (url === '/v1/libraries/conflicts-lib/conflicts/conflict-1/resolve' && init?.method === 'POST') {
        resolved = true;
        return json({ ...conflict('conflict-1'), status: 'resolved', resolved_at: 'now' });
      }
      if (url.startsWith('/v1/libraries/conflicts-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Conflict/ }));
    await userEvent.click(screen.getByRole('tab', { name: 'Versions' }));
    expect(await screen.findByText(/Discovered 2026-05-28T12:00:00Z/)).toBeInTheDocument();
    expect(screen.getByText('Sibling conflict.sibling.md')).toBeInTheDocument();
    await userEvent.click(await screen.findByLabelText('Resolve conflict conflict-1'));

    await waitFor(() => expect(screen.queryByText('conflict.md open')).not.toBeInTheDocument());
    expect(fetch).toHaveBeenCalledWith(
      '/v1/libraries/conflicts-lib/conflicts/conflict-1/resolve',
      expect.objectContaining({ method: 'POST' })
    );
  });

  it('resolves a conflict by saving the chosen ours content before marking resolved', async () => {
    let resolved = false;
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-merge', slug: 'merge-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/merge-lib/documents') {
        return json([
          {
            id: 'doc-merge',
            path: 'conflict.md',
            head_version_id: 'head',
            content_type: 'text/markdown',
            byte_size: 10,
            metadata: { title: 'Conflict' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/merge-lib/documents/conflict.md' && init?.method !== 'PUT') {
        return new Response('# Head', { headers: { ETag: '"head"', 'content-type': 'text/markdown' } });
      }
      if (url === '/v1/libraries/merge-lib/documents/conflict.md' && init?.method === 'PUT') {
        expect(init.headers).toMatchObject({
          'If-Match': '"head"',
          'X-Quarry-Transaction-Actor': 'Avery',
        });
        expect(init.body).toBe('# Ours');
        return json({ version: { id: 'merged' } }, { ETag: '"merged"' });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'conflict.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/merge-lib/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions/ours')) {
        return json({ version: version('ours'), content: '# Ours' });
      }
      if (url === '/v1/libraries/merge-lib/documents/conflict.sibling.md/versions/theirs') {
        return json({ version: version('theirs'), content: '# Theirs' });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url === '/v1/libraries/merge-lib/conflicts') {
        return json(resolved ? [] : [conflict('conflict-merge')]);
      }
      if (url === '/v1/libraries/merge-lib/conflicts/conflict-merge/resolve' && init?.method === 'POST') {
        resolved = true;
        return json({ ...conflict('conflict-merge'), status: 'resolved', resolved_at: 'now' });
      }
      if (url === '/v1/libraries/merge-lib/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/merge-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);
    localStorage.setItem('quarry:author', 'Avery');

    renderApp({ seedAuthor: false });

    await userEvent.click(await screen.findByRole('treeitem', { name: /Conflict/ }));
    await userEvent.click(screen.getByRole('tab', { name: 'Versions' }));
    await userEvent.click(await screen.findByLabelText('Open conflict conflict-merge'));

    await waitFor(() => expect(screen.getAllByText('# Ours').length).toBeGreaterThan(0));
    expect(screen.getByText('# Theirs')).toBeInTheDocument();
    expect(fetch).toHaveBeenCalledWith(
      '/v1/libraries/merge-lib/documents/conflict.sibling.md/versions/theirs',
      undefined
    );

    await userEvent.click(screen.getByRole('button', { name: 'Use ours' }));

    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Resolve conflict' })).not.toBeInTheDocument());
    expect(fetch).toHaveBeenCalledWith(
      '/v1/libraries/merge-lib/documents/conflict.md',
      expect.objectContaining({ method: 'PUT' })
    );
    expect(fetch).toHaveBeenCalledWith(
      '/v1/libraries/merge-lib/conflicts/conflict-merge/resolve',
      expect.objectContaining({ method: 'POST' })
    );
  });

  it('previews images and shows metadata for unknown binary documents without opening the editor', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-6', slug: 'media-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/media-lib/documents') {
        return json([
          {
            id: 'doc-image',
            path: 'assets/photo.png',
            head_version_id: 'v-img',
            content_type: 'image/png',
            byte_size: 2048,
            metadata: {},
            updated_at: 'now',
          },
          {
            id: 'doc-gallery',
            path: 'gallery.md',
            head_version_id: 'v-gallery',
            content_type: 'text/markdown',
            byte_size: 38,
            metadata: { title: 'Gallery' },
            updated_at: 'now',
          },
          {
            id: 'doc-binary',
            path: 'archives/raw.bin',
            head_version_id: 'v-bin',
            content_type: 'application/octet-stream',
            byte_size: 4096,
            content_hash: 'blake3-raw-bin',
            metadata: {},
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/media-lib/documents/assets/photo.png') {
        return new Response('image-bytes', { headers: { ETag: '"v-img"', 'content-type': 'image/png' } });
      }
      if (url === '/v1/libraries/media-lib/documents/gallery.md') {
        return new Response('# Gallery\n\n![Project photo](assets/photo.png)', {
          headers: { ETag: '"v-gallery"', 'content-type': 'text/markdown' },
        });
      }
      if (url === '/v1/libraries/media-lib/documents/archives/raw.bin') {
        return new Response('binary-bytes', {
          headers: { ETag: '"v-bin"', 'content-type': 'application/octet-stream' },
        });
      }
      if (url.endsWith('/gallery.md/outgoing-links')) {
        return json({
          path: 'gallery.md',
          links: [
            link({
              target_kind: 'markdown_link',
              target_text: 'assets/photo.png',
              target_path: 'assets/photo.png',
              resolved: true,
            }),
          ],
        });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'media', links: [] });
      }
      if (url.startsWith('/v1/libraries/media-lib/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url === '/v1/libraries/media-lib/conflicts') return json([]);
      if (url.startsWith('/v1/libraries/media-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /photo.png/ }));
    const image = await screen.findByRole('img', { name: 'assets/photo.png preview' });
    expect(image).toHaveAttribute('src', '/v1/libraries/media-lib/documents/assets/photo.png');
    expect(screen.queryByLabelText('Plate markdown editor')).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole('treeitem', { name: /raw.bin/ }));
    const binaryPreview = await screen.findByRole('region', { name: 'Binary document preview' });
    expect(within(binaryPreview).getByText('application/octet-stream')).toBeInTheDocument();
    expect(within(binaryPreview).getByText('4 KB')).toBeInTheDocument();
    expect(within(binaryPreview).getByText('Hash')).toBeInTheDocument();
    expect(within(binaryPreview).getByText('blake3-raw-bin')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Download' })).toHaveAttribute(
      'href',
      '/v1/libraries/media-lib/documents/archives/raw.bin'
    );
    expect(screen.queryByLabelText('Plate markdown editor')).not.toBeInTheDocument();
  });

  it('opens the command palette from the keyboard and quick-opens documents', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-7', slug: 'palette-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/palette-lib/documents') {
        return json([
          {
            id: 'doc-daily',
            path: 'daily.md',
            head_version_id: 'v-daily',
            content_type: 'text/markdown',
            byte_size: 7,
            metadata: { title: 'Daily' },
            updated_at: 'now',
          },
          {
            id: 'doc-guide',
            path: 'guide.md',
            head_version_id: 'v-guide',
            content_type: 'text/markdown',
            byte_size: 7,
            metadata: { title: 'Guide' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/palette-lib/documents/daily.md') {
        return new Response('# Daily', { headers: { ETag: '"v-daily"', 'content-type': 'text/markdown' } });
      }
      if (url === '/v1/libraries/palette-lib/documents/guide.md') {
        return new Response('# Guide', { headers: { ETag: '"v-guide"', 'content-type': 'text/markdown' } });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'document.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/palette-lib/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url === '/v1/libraries/palette-lib/conflicts') return json([]);
      if (url.startsWith('/v1/libraries/palette-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await screen.findByRole('treeitem', { name: /Daily/ });
    await userEvent.keyboard('{Control>}k{/Control}');
    await userEvent.type(await screen.findByRole('combobox', { name: 'Command palette' }), 'guide');
    await userEvent.click(await screen.findByText('Open Guide'));

    await waitFor(() => expect(screen.getByLabelText('Plate markdown editor')).toHaveTextContent('Guide'));
  });

  it('opens workspace settings from the command palette', async () => {
    localStorage.setItem('quarry:layout:settings-lib', '[20,55,25]');
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-settings', slug: 'settings-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/settings-lib/documents') return json([]);
      if (url === '/v1/libraries/settings-lib/conflicts') return json([]);
      if (url === '/v1/libraries/settings-lib/git/peers') return json([]);
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await screen.findByRole('combobox', { name: 'Library switcher' });
    await userEvent.keyboard('{Control>}k{/Control}');
    await userEvent.click(await screen.findByText('Open settings'));
    const settings = screen.getByRole('dialog', { name: 'Workspace settings' });
    expect(within(settings).getByText('settings-lib')).toBeInTheDocument();
    expect(within(settings).getByText('quarry:layout:settings-lib')).toBeInTheDocument();

    await userEvent.click(within(settings).getByRole('button', { name: 'Use light theme' }));
    expect(screen.getByRole('main')).toHaveAttribute('data-theme', 'light');
    expect(localStorage.getItem('quarry:theme')).toBe('light');

    await userEvent.click(within(settings).getByRole('button', { name: 'Use dark theme' }));
    expect(screen.getByRole('main')).toHaveAttribute('data-theme', 'dark');
    expect(localStorage.getItem('quarry:theme')).toBe('dark');

    await userEvent.click(within(settings).getByRole('button', { name: 'Reset workspace layout' }));
    expect(localStorage.getItem('quarry:layout:settings-lib')).toBeNull();

    await userEvent.click(within(settings).getByRole('button', { name: 'Close settings' }));
    expect(screen.queryByRole('dialog', { name: 'Workspace settings' })).not.toBeInTheDocument();
  });

  it('copies the FUSE mount command from the command palette', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    });
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-fuse', slug: 'fuse-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/fuse-lib/documents') return json([]);
      if (url === '/v1/libraries/fuse-lib/conflicts') return json([]);
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await screen.findByRole('combobox', { name: 'Library switcher' });
    await userEvent.keyboard('{Control>}k{/Control}');
    await userEvent.click(await screen.findByText('Copy FUSE mount command'));

    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith('mkdir -p fuse-lib && quarry mount fuse-lib fuse-lib')
    );
  });

  it('traps settings dialog focus', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-focus', slug: 'focus-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/focus-lib/documents') return json([]);
      if (url === '/v1/libraries/focus-lib/conflicts') return json([]);
      if (url === '/v1/libraries/focus-lib/git/peers') return json([]);
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await screen.findByRole('combobox', { name: 'Library switcher' });
    await userEvent.keyboard('{Control>}k{/Control}');
    await userEvent.click(await screen.findByText('Open settings'));
    const settings = screen.getByRole('dialog', { name: 'Workspace settings' });
    const close = within(settings).getByRole('button', { name: 'Close settings' });
    const resetLayout = within(settings).getByRole('button', { name: 'Reset workspace layout' });

    await waitFor(() => expect(close).toHaveFocus());
    await userEvent.tab({ shift: true });
    expect(resetLayout).toHaveFocus();
    await userEvent.tab();
    expect(close).toHaveFocus();

    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('dialog', { name: 'Workspace settings' })).not.toBeInTheDocument();
  });

  it('supports keyboard selection and preview for server search results', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-search-key', slug: 'search-key-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/search-key-lib/documents') {
        return json([
          {
            id: 'doc-guide',
            path: 'guide.md',
            head_version_id: 'v-guide',
            content_type: 'text/markdown',
            byte_size: 7,
            metadata: { title: 'Guide' },
            updated_at: 'now',
          },
          {
            id: 'doc-journal',
            path: 'journal.md',
            head_version_id: 'v-journal',
            content_type: 'text/markdown',
            byte_size: 9,
            metadata: { title: 'Journal' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/search-key-lib/documents/journal.md') {
        return new Response('# Journal', { headers: { ETag: '"v-journal"', 'content-type': 'text/markdown' } });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'document.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/search-key-lib/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url === '/v1/libraries/search-key-lib/conflicts') return json([]);
      if (url === '/v1/libraries/search-key-lib/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/search-key-lib/search')) {
        return json({
          results: [
            {
              document_id: 'doc-guide',
              path: 'guide.md',
              title: 'Guide',
              content_type: 'text/markdown',
              score: 1,
              snippet: 'Guide snippet',
              matched_fields: ['title'],
              head_version_id: 'v-guide',
            },
            {
              document_id: 'doc-journal',
              path: 'journal.md',
              title: 'Journal',
              content_type: 'text/markdown',
              score: 0.8,
              snippet: 'Journal snippet',
              matched_fields: ['body'],
              head_version_id: 'v-journal',
            },
          ],
          cursor: null,
        });
      }
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('button', { name: 'Search' }));
    await userEvent.clear(await screen.findByPlaceholderText('Search documents…'));
    await userEvent.type(screen.getByPlaceholderText('Search documents…'), 'guide');
    const results = await screen.findByRole('listbox', { name: 'Search results' });
    expect(screen.getByLabelText('Search result preview')).toHaveTextContent('Guide snippet');

    results.focus();
    await userEvent.keyboard('{ArrowDown}');
    expect(screen.getByLabelText('Search result preview')).toHaveTextContent('Journal snippet');

    await userEvent.keyboard('{Enter}');

    await waitFor(() => expect(screen.getByLabelText('Plate markdown editor')).toHaveTextContent('Journal'));
    expect(window.location.pathname).toBe('/lib/search-key-lib/documents/journal.md');
  });

  it('runs Git peer sync as an explicit operation with pending and result state', async () => {
    let resolveSync: (response: Response) => void = () => {};
    const syncResponse = new Promise<Response>((resolve) => {
      resolveSync = resolve;
    });
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-8', slug: 'git-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/git-lib/documents') return json([]);
      if (url === '/v1/libraries/git-lib/conflicts') return json([]);
      if (url === '/v1/libraries/git-lib/git/peers') {
        return json([
          {
            id: 'peer-1',
            library_id: 'lib-8',
            kind: 'git',
            config: { repo: '/tmp/notes', branch: 'main', remote: 'origin' },
          },
        ]);
      }
      if (url === '/v1/libraries/git-lib/git/peers/peer-1/sync' && init?.method === 'POST') {
        return syncResponse;
      }
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await screen.findByRole('combobox', { name: 'Library switcher' });
    await userEvent.keyboard('{Control>}k{/Control}');
    await userEvent.click(await screen.findByText('Sync with Git peer'));
    await userEvent.click(await screen.findByRole('button', { name: 'Sync peer peer-1' }));

    expect(screen.getByText('Running sync...')).toBeInTheDocument();

    resolveSync(
      json({
        imported_paths: ['from-git.md'],
        exported_paths: ['to-git.md', 'also-to-git.md'],
        conflict_paths: ['conflicted.md'],
        conflicts: [],
        commit_id: 'abc123',
      })
    );

    expect(await screen.findByText('Imported 1 · Exported 2 · Conflicts 1')).toBeInTheDocument();
    expect(fetch).toHaveBeenCalledWith(
      '/v1/libraries/git-lib/git/peers/peer-1/sync',
      expect.objectContaining({ method: 'POST' })
    );
  });

  it('persists the theme preference and uses a per-library layout storage key', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-9', slug: 'theme-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/theme-lib/documents') return json([]);
      if (url === '/v1/libraries/theme-lib/conflicts') return json([]);
      if (url === '/v1/libraries/theme-lib/git/peers') return json([]);
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    const { unmount } = renderApp();

    expect(await screen.findByLabelText('Workspace layout')).toHaveAttribute(
      'data-layout-storage-key',
      'quarry:layout:theme-lib'
    );
    await userEvent.keyboard('{Control>}k{/Control}');
    await userEvent.click(await screen.findByText('Switch to light theme'));
    expect(screen.getByRole('main')).toHaveAttribute('data-theme', 'light');
    expect(localStorage.getItem('quarry:theme')).toBe('light');

    unmount();
    renderApp();

    expect(await screen.findByRole('main')).toHaveAttribute('data-theme', 'light');
  });

  it('downloads the canonical document bytes from the command palette', async () => {
    let documentFetches = 0;
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-dl', slug: 'dl-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/dl-lib/documents') {
        return json([
          {
            id: 'doc-dl',
            path: 'notes/readme.md',
            head_version_id: 'v1',
            content_type: 'text/markdown',
            byte_size: 12,
            metadata: { title: 'Readme' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/dl-lib/documents/notes/readme.md') {
        documentFetches += 1;
        // The first fetch loads the editor; later fetches are download-time
        // reads and serve distinct bytes so the test can prove the download
        // came from the canonical API export, not the editor's local mirror.
        const body = documentFetches === 1 ? '# Readme\nBody' : '---\ntitle: Readme\n---\n\n# Readme\nBody\n';
        return new Response(body, { headers: { ETag: '"v1"', 'content-type': 'text/markdown' } });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'notes/readme.md', links: [] });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url.startsWith('/v1/libraries/dl-lib/graph')) return json({ nodes: [], edges: [], truncated: false });
      if (url === '/v1/libraries/dl-lib/conflicts') return json([]);
      if (url === '/v1/libraries/dl-lib/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/dl-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);
    const downloadedBlobs: Blob[] = [];
    const createObjectURL = vi.fn((blob: Blob) => {
      downloadedBlobs.push(blob);
      return 'blob:mock';
    });
    URL.createObjectURL = createObjectURL;
    URL.revokeObjectURL = vi.fn();
    let downloadName = '';
    const click = vi
      .spyOn(HTMLAnchorElement.prototype, 'click')
      .mockImplementation(function (this: HTMLAnchorElement) {
        downloadName = this.download;
      });

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Readme/ }));
    await screen.findByLabelText('Plate markdown editor');
    await userEvent.keyboard('{Control>}k{/Control}');
    await userEvent.click(await screen.findByText('Download as Markdown'));

    await waitFor(() => expect(createObjectURL).toHaveBeenCalled());
    expect(downloadName).toBe('readme.md');
    expect(downloadedBlobs).toHaveLength(1);
    // Which realm the blob comes from depends on the Node version running
    // vitest: Node's Blob exposes .text() but is rejected by jsdom's
    // FileReader, while jsdom's Blob only supports FileReader.
    const downloadedBlob = downloadedBlobs[0];
    const downloadedText =
      typeof downloadedBlob.text === 'function'
        ? await downloadedBlob.text()
        : await new Promise((resolve, reject) => {
            const reader = new FileReader();
            reader.onload = () => resolve(String(reader.result));
            reader.onerror = () => reject(reader.error);
            reader.readAsText(downloadedBlob);
          });
    expect(downloadedText).toBe('---\ntitle: Readme\n---\n\n# Readme\nBody\n');
    click.mockRestore();
  });

  it('copies the selected library document raw link from the toolbar menu', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    });
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-copy', slug: 'copy-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/copy-lib/documents') {
        return json([
          {
            id: 'doc-copy',
            path: 'notes/raw.md',
            head_version_id: 'v-copy',
            content_type: 'text/markdown',
            byte_size: 8,
            metadata: { title: 'Raw Link' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/copy-lib/documents/notes/raw.md') {
        return new Response('# Raw\n', { headers: { ETag: '"v-copy"', 'content-type': 'text/markdown' } });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'notes/raw.md', links: [] });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url.startsWith('/v1/libraries/copy-lib/graph')) return json({ nodes: [], edges: [], truncated: false });
      if (url === '/v1/libraries/copy-lib/conflicts') return json([]);
      if (url === '/v1/libraries/copy-lib/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/copy-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Raw Link/ }));
    fireEvent.pointerDown(screen.getByRole('button', { name: 'Document actions' }));
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Copy raw link' }));

    expect(writeText).toHaveBeenCalledWith('http://127.0.0.1/v1/libraries/copy-lib/documents/notes/raw.md');
  });

  it('copies the selected tmp document raw link from the toolbar menu', async () => {
    const secret = '72cb58585aa73e35758bc1141f79e32e';
    window.history.pushState({}, '', `/tmp/${secret}`);
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    });
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') return json([]);
      if (url === `/v1/tmp/documents/${secret}`) {
        return new Response('# Tmp\n', {
          headers: {
            ETag: '"tmp-copy"',
            'content-type': 'text/markdown',
            'x-quarry-document-id': 'tmp-copy',
            'x-quarry-expires-at': '2099-01-01T00:00:00Z',
          },
        });
      }
      if (url === `/v1/tmp/documents/${secret}/presence`) return json({ presence: [] });
      if (url === `/v1/tmp/documents/${secret}/review?includeResolved=1`) {
        return json({ documentId: 'tmp-copy', comments: [], suggestions: [], conflicts: [] });
      }
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    expect(await screen.findByRole('button', { name: 'Document mode' })).toHaveTextContent('Editing');
    fireEvent.pointerDown(screen.getByRole('button', { name: 'Document actions' }));
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Copy raw link' }));

    expect(writeText).toHaveBeenCalledWith(`http://127.0.0.1/v1/tmp/documents/${secret}`);
  });

  it('shows Upload Markdown for markdown documents and hides it for raw text documents', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-upload-menu', slug: 'upload-menu', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/upload-menu/documents') {
        return json([
          {
            id: 'doc-md',
            path: 'daily.md',
            head_version_id: 'v-md',
            content_type: 'text/markdown',
            byte_size: 8,
            metadata: { title: 'Daily' },
            updated_at: 'now',
          },
          {
            id: 'doc-txt',
            path: 'raw.txt',
            head_version_id: 'v-txt',
            content_type: 'text/plain',
            byte_size: 4,
            metadata: { title: 'Raw' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/upload-menu/documents/daily.md') {
        return new Response('# Daily\n', { headers: { ETag: '"v-md"', 'content-type': 'text/markdown' } });
      }
      if (url === '/v1/libraries/upload-menu/documents/raw.txt') {
        return new Response('raw\n', { headers: { ETag: '"v-txt"', 'content-type': 'text/plain' } });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) return json({ path: 'daily.md', links: [] });
      if (url.endsWith('/versions')) return json([]);
      if (url.startsWith('/v1/libraries/upload-menu/graph')) return json({ nodes: [], edges: [], truncated: false });
      if (url === '/v1/libraries/upload-menu/conflicts') return json([]);
      if (url === '/v1/libraries/upload-menu/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/upload-menu/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Daily/ }));
    fireEvent.pointerDown(screen.getByRole('button', { name: 'Document actions' }));
    expect(await screen.findByText('Upload Markdown')).toBeInTheDocument();
    await userEvent.keyboard('{Escape}');

    await userEvent.click(screen.getByRole('treeitem', { name: /Raw/ }));
    fireEvent.pointerDown(screen.getByRole('button', { name: 'Document actions' }));
    expect(screen.queryByText('Upload Markdown')).not.toBeInTheDocument();
  });

  it('uploads a markdown file to the selected library document using a fresh ETag', async () => {
    let documentFetches = 0;
    let putInit: RequestInit | undefined;
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-upload', slug: 'upload-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/upload-lib/documents') {
        return json([
          {
            id: 'doc-upload',
            path: 'notes/readme.md',
            head_version_id: 'v1',
            content_type: 'text/markdown',
            byte_size: 10,
            metadata: { title: 'Readme' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/upload-lib/documents/notes/readme.md' && init?.method === 'PUT') {
        putInit = init;
        return json(writeOutcome('doc-upload', 'v3', 'notes/readme.md'), { ETag: '"v3"' });
      }
      if (url === '/v1/libraries/upload-lib/documents/notes/readme.md') {
        documentFetches += 1;
        const etag = documentFetches === 1 ? '"v1"' : '"v2"';
        const body = documentFetches >= 3 ? '# Uploaded\n' : '# Current\n';
        return new Response(body, {
          headers: {
            ETag: etag,
            'content-type': 'text/markdown',
          },
        });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'notes/readme.md', links: [] });
      }
      if (url.endsWith('/review?includeResolved=1')) {
        return json({ documentId: 'doc-upload', comments: [], suggestions: [], conflicts: [] });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url.startsWith('/v1/libraries/upload-lib/graph')) return json({ nodes: [], edges: [], truncated: false });
      if (url === '/v1/libraries/upload-lib/conflicts') return json([]);
      if (url === '/v1/libraries/upload-lib/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/upload-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Readme/ }));
    await waitFor(() => expect(screen.getByLabelText('Plate markdown editor')).toHaveTextContent('Current'));
    fireEvent.pointerDown(screen.getByRole('button', { name: 'Document actions' }));
    await userEvent.click(await screen.findByText('Upload Markdown'));
    await userEvent.upload(
      screen.getByLabelText('Upload Markdown file'),
      markdownFile('# Uploaded\n')
    );

    await waitFor(() => expect(putInit).toBeTruthy());
    expect(putInit?.body).toBe('# Uploaded\n');
    expect(putInit?.headers).toMatchObject({
      'If-Match': '"v2"',
      'content-type': 'text/markdown',
      'X-Quarry-Origin-Id': expect.any(String),
      'X-Quarry-Transaction-Actor': 'Tester',
    });
    await waitFor(() => expect(fetch).toHaveBeenCalledWith('/v1/libraries/upload-lib/documents/notes/readme.md/versions', undefined));
  });

  it('uploads markdown in tmp mode through the tmp document endpoint', async () => {
    const secret = '72cb58585aa73e35758bc1141f79e32e';
    window.history.pushState({}, '', `/tmp/${secret}`);
    let putUrl = '';
    let putInit: RequestInit | undefined;
    let documentFetches = 0;
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/v1/libraries') return json([]);
      if (url === `/v1/tmp/documents/${secret}` && init?.method === 'PUT') {
        putUrl = url;
        putInit = init;
        return json(writeOutcome('tmp-upload', 'tmp-v3', secret), { ETag: '"tmp-v3"' });
      }
      if (url === `/v1/tmp/documents/${secret}`) {
        documentFetches += 1;
        return new Response(documentFetches >= 3 ? '# Tmp uploaded\n' : '# Tmp\n', {
          headers: {
            ETag: documentFetches === 1 ? '"tmp-v1"' : '"tmp-v2"',
            'content-type': 'text/markdown',
          },
        });
      }
      if (url === `/v1/tmp/documents/${secret}/presence`) return json({ presence: [] });
      if (url === `/v1/tmp/documents/${secret}/review?includeResolved=1`) {
        return json({ documentId: 'tmp-upload', comments: [], suggestions: [], conflicts: [] });
      }
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    expect(await screen.findByRole('button', { name: 'Document mode' })).toHaveTextContent('Editing');
    await userEvent.keyboard('{Control>}k{/Control}');
    await userEvent.click(await screen.findByText('Upload Markdown'));
    await userEvent.upload(
      screen.getByLabelText('Upload Markdown file'),
      markdownFile('# Tmp uploaded\n')
    );

    await waitFor(() => expect(putInit).toBeTruthy());
    expect(putUrl).toBe(`/v1/tmp/documents/${secret}`);
    expect(putInit?.body).toBe('# Tmp uploaded\n');
    expect(putInit?.headers).toMatchObject({
      'If-Match': '"tmp-v2"',
      'content-type': 'text/markdown',
      'X-Quarry-Origin-Id': expect.any(String),
      'X-Quarry-Transaction-Actor': 'Tester',
    });
    expect(fetch).not.toHaveBeenCalledWith('/v1/tmp/documents', undefined);
  });

  it('blocks markdown upload while the live document has unsaved session state', async () => {
    const alert = vi.spyOn(window, 'alert').mockImplementation(() => {});
    const clickFileInput = vi.spyOn(HTMLInputElement.prototype, 'click');
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-blocked', slug: 'blocked-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/blocked-lib/documents') {
        return json([
          {
            id: 'doc-blocked',
            path: 'blocked.md',
            head_version_id: 'v1',
            content_type: 'text/markdown',
            byte_size: 10,
            metadata: { title: 'Blocked' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/blocked-lib/documents/blocked.md') {
        return new Response('# Blocked\n', {
          headers: {
            ETag: '"v1"',
            'content-type': 'text/markdown',
            'x-quarry-document-id': 'doc-blocked',
          },
        });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) return json({ path: 'blocked.md', links: [] });
      if (url.endsWith('/review?includeResolved=1')) {
        return json({ documentId: 'doc-blocked', comments: [], suggestions: [], conflicts: [] });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url.startsWith('/v1/libraries/blocked-lib/graph')) return json({ nodes: [], edges: [], truncated: false });
      if (url === '/v1/libraries/blocked-lib/conflicts') return json([]);
      if (url === '/v1/libraries/blocked-lib/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/blocked-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Blocked/ }));
    expect(await screen.findByLabelText('Save status')).toHaveTextContent(/Reconnecting|Saving/);
    fireEvent.pointerDown(screen.getByRole('button', { name: 'Document actions' }));
    await userEvent.click(await screen.findByText('Upload Markdown'));

    expect(alert).toHaveBeenCalledWith(expect.stringMatching(/finish saving/i));
    expect(clickFileInput).not.toHaveBeenCalled();
  });

  it('alerts on upload failure and leaves the visible document content unchanged', async () => {
    const alert = vi.spyOn(window, 'alert').mockImplementation(() => {});
    let documentFetches = 0;
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-fail', slug: 'fail-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/fail-lib/documents') {
        return json([
          {
            id: 'doc-fail',
            path: 'fail.md',
            head_version_id: 'v1',
            content_type: 'text/markdown',
            byte_size: 10,
            metadata: { title: 'Fail' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/fail-lib/documents/fail.md' && init?.method === 'PUT') {
        return new Response(JSON.stringify({ error: 'write failed' }), {
          status: 500,
          headers: { 'content-type': 'application/json' },
        });
      }
      if (url === '/v1/libraries/fail-lib/documents/fail.md') {
        documentFetches += 1;
        return new Response('# Current\n', {
          headers: {
            ETag: documentFetches === 1 ? '"v1"' : '"v2"',
            'content-type': 'text/markdown',
          },
        });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) return json({ path: 'fail.md', links: [] });
      if (url.endsWith('/versions')) return json([]);
      if (url.startsWith('/v1/libraries/fail-lib/graph')) return json({ nodes: [], edges: [], truncated: false });
      if (url === '/v1/libraries/fail-lib/conflicts') return json([]);
      if (url === '/v1/libraries/fail-lib/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/fail-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Fail/ }));
    const editor = await screen.findByLabelText('Plate markdown editor');
    expect(editor).toHaveTextContent('Current');
    fireEvent.pointerDown(screen.getByRole('button', { name: 'Document actions' }));
    await userEvent.click(await screen.findByText('Upload Markdown'));
    await userEvent.upload(
      screen.getByLabelText('Upload Markdown file'),
      markdownFile('# Replacement\n')
    );

    await waitFor(() => expect(alert).toHaveBeenCalledWith(expect.stringContaining('write failed')));
    expect(editor).toHaveTextContent('Current');
  });

  it('requires a name on first run and stamps it into the author store', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-1', slug: 'notes', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/notes/documents') return json([]);
      if (url === '/v1/libraries/notes/conflicts') return json([]);
      if (url === '/v1/libraries/notes/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/notes/search')) return json({ results: [], cursor: null });
      if (url.startsWith('/v1/libraries/notes/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp({ seedAuthor: false });

    const dialog = await screen.findByRole('dialog', { name: 'Welcome to Quarry' });
    const getStarted = within(dialog).getByRole('button', { name: 'Get started' });
    expect(getStarted).toBeDisabled();

    // Whitespace does not count as a name.
    await userEvent.type(within(dialog).getByLabelText('Your name'), '   ');
    expect(getStarted).toBeDisabled();

    // The reserved default author name does not count either.
    await userEvent.clear(within(dialog).getByLabelText('Your name'));
    await userEvent.type(within(dialog).getByLabelText('Your name'), 'user');
    expect(getStarted).toBeDisabled();

    await userEvent.clear(within(dialog).getByLabelText('Your name'));
    await userEvent.type(within(dialog).getByLabelText('Your name'), '  Avery  ');
    expect(getStarted).toBeEnabled();
    await userEvent.click(getStarted);

    expect(screen.queryByRole('dialog', { name: 'Welcome to Quarry' })).not.toBeInTheDocument();
    expect(localStorage.getItem('quarry:author')).toBe('Avery');
  });

  it('does not show onboarding when an author is already stored', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-1', slug: 'notes', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/notes/documents') return json([]);
      if (url === '/v1/libraries/notes/conflicts') return json([]);
      if (url === '/v1/libraries/notes/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/notes/search')) return json({ results: [], cursor: null });
      if (url.startsWith('/v1/libraries/notes/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await waitFor(() => expect(fetch).toHaveBeenCalled());
    expect(screen.queryByRole('dialog', { name: 'Welcome to Quarry' })).not.toBeInTheDocument();
  });
});

function renderApp({ seedAuthor = true }: { seedAuthor?: boolean } = {}) {
  // Most tests predate onboarding; a stored author keeps the modal away.
  if (seedAuthor) localStorage.setItem('quarry:author', 'Tester');
  return render(
    <SWRConfig value={{ provider: () => new Map() }}>
      <App />
    </SWRConfig>
  );
}

function json(body: unknown, headers: Record<string, string> = {}) {
  return new Response(JSON.stringify(body), {
    headers: { 'content-type': 'application/json', ...headers },
  });
}

function markdownFile(content: string, name = 'upload.md') {
  const file = new File([content], name, { type: 'text/markdown' });
  Object.defineProperty(file, 'text', {
    value: vi.fn().mockResolvedValue(content),
  });
  return file;
}

function link(overrides: Record<string, unknown>) {
  const resolved = overrides.resolved ?? true;
  return {
    src_doc_id: 'doc-1',
    src_version_id: 'v1',
    src_path: 'daily.md',
    target_kind: 'wiki_link',
    target_text: 'Guide',
    target_doc_id: 'doc-2',
    target_path: 'guide.md',
    target_anchor: null,
    alias: null,
    start_offset: 0,
    end_offset: 10,
    resolved,
    resolution_status: resolved ? 'resolved' : 'unresolved',
    ...overrides,
  };
}

function version(id: string) {
  return {
    id,
    document_id: 'doc-versions',
    tx_id: `tx-${id}`,
    content_hash: null,
    inline_content: null,
    metadata: {},
    content_type: 'text/markdown',
    byte_size: 12,
    created_at: `2026-05-28T12:00:0${id.slice(1)}Z`,
  };
}

function historyEntry(id: string) {
  return {
    id,
    document_id: 'doc-versions',
    latest_version_id: id,
    earliest_version_id: id,
    raw_version_count: 1,
    source: 'rest',
    actor: null,
    message: null,
    provenance: {},
    checkpoint_reason: null,
    content_type: 'text/markdown',
    byte_size: 12,
    created_at: `2026-05-28T12:00:0${id.slice(1)}Z`,
    updated_at: `2026-05-28T12:00:0${id.slice(1)}Z`,
  };
}

function writeOutcome(documentId: string, versionId: string, path: string) {
  const versionRecord = {
    id: versionId,
    document_id: documentId,
    tx_id: `tx-${versionId}`,
    content_hash: null,
    inline_content: null,
    metadata: { content_type: 'text/markdown' },
    content_type: 'text/markdown',
    byte_size: 11,
    created_at: '2026-05-28T12:00:00Z',
  };
  return {
    document: {
      id: documentId,
      library_id: 'lib-cache',
      path,
      metadata: { content_type: 'text/markdown' },
      version: versionRecord,
      content: '# Untitled\n',
      created_at: '2026-05-28T12:00:00Z',
      updated_at: '2026-05-28T12:00:00Z',
    },
    version: versionRecord,
    transaction: {
      id: `tx-${versionId}`,
      library_id: 'lib-cache',
      state: 'committed',
      actor: null,
      source: 'rest',
      message: null,
      provenance: {},
      created_at: '2026-05-28T12:00:00Z',
      committed_at: '2026-05-28T12:00:00Z',
    },
  };
}

function conflict(id: string) {
  return {
    id,
    library_id: 'lib-5',
      path: 'conflict.md',
      conflict_path: 'conflict.sibling.md',
      ours_version_id: 'ours',
      theirs_version_id: 'theirs',
      status: 'open',
    discovered_at: '2026-05-28T12:00:00Z',
    resolved_at: null,
  };
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
