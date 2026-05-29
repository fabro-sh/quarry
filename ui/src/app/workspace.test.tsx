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

  it('loads a library, opens a markdown document, and saves with the current ETag', async () => {
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
    expect(within(screen.getByRole('contentinfo')).getByText('Clean · "v1"')).toBeInTheDocument();
    await userEvent.clear(await screen.findByLabelText('Markdown source'));
    await userEvent.type(screen.getByLabelText('Markdown source'), '# Daily updated');
    await userEvent.click(screen.getByRole('button', { name: 'Save document' }));

    await waitFor(() => expect(screen.getByText('Saved')).toBeInTheDocument());
    expect(within(screen.getByRole('contentinfo')).getByText('Saved · "v2"')).toBeInTheDocument();
  });

  it('refreshes indexed document state after saving markdown changes', async () => {
    let saved = false;
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-save-index', slug: 'save-index-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/save-index-lib/documents') {
        return json([
          {
            id: 'doc-source',
            path: 'source.md',
            head_version_id: saved ? 'v2' : 'v1',
            content_type: 'text/markdown',
            byte_size: saved ? 26 : 9,
            metadata: { title: 'Source' },
            updated_at: 'now',
          },
          {
            id: 'doc-target',
            path: 'target.md',
            head_version_id: 'v-target',
            content_type: 'text/markdown',
            byte_size: 8,
            metadata: { title: 'Target' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/save-index-lib/documents/source.md' && init?.method === 'PUT') {
        saved = true;
        return json({ version: { id: 'v2' } }, { ETag: '"v2"' });
      }
      if (url === '/v1/libraries/save-index-lib/documents/source.md') {
        return new Response(saved ? 'See [Target](target.md)' : 'No links.', {
          headers: { ETag: saved ? '"v2"' : '"v1"', 'content-type': 'text/markdown' },
        });
      }
      if (url.endsWith('/outgoing-links')) {
        return json({
          path: 'source.md',
          links: saved
            ? [
                link({
                  target_kind: 'markdown_link',
                  target_text: 'target.md',
                  target_path: 'target.md',
                  resolved: true,
                }),
              ]
            : [],
        });
      }
      if (url.endsWith('/backlinks')) return json({ path: 'source.md', links: [] });
      if (url.startsWith('/v1/libraries/save-index-lib/graph')) {
        return json({
          nodes: saved
            ? [
                { id: 'doc-source', path: 'source.md', title: 'Source', content_type: 'text/markdown' },
                { id: 'doc-target', path: 'target.md', title: 'Target', content_type: 'text/markdown' },
              ]
            : [],
          edges: saved
            ? [
                {
                  id: 'edge-source-target',
                  source: 'doc-source',
                  source_path: 'source.md',
                  target: 'doc-target',
                  target_path: 'target.md',
                  target_kind: 'markdown_link',
                  target_text: 'target.md',
                  resolved: true,
                },
              ]
            : [],
          truncated: false,
        });
      }
      if (url.endsWith('/versions')) return json(saved ? [version('v2'), version('v1')] : [version('v1')]);
      if (url === '/v1/libraries/save-index-lib/conflicts') return json([]);
      if (url === '/v1/libraries/save-index-lib/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/save-index-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Source/ }));
    await waitFor(() => expect(screen.queryByRole('button', { name: 'target.md' })).not.toBeInTheDocument());
    await userEvent.clear(await screen.findByLabelText('Markdown source'));
    await userEvent.type(screen.getByLabelText('Markdown source'), 'See [Target](target.md)');
    await userEvent.click(screen.getByRole('button', { name: 'Save document' }));

    expect(await screen.findByRole('button', { name: 'target.md' })).toBeInTheDocument();
    await userEvent.click(screen.getByRole('tab', { name: 'Graph' }));
    expect(await screen.findByText('source.md -> target.md')).toBeInTheDocument();
  });

  it('traps stale-save conflict dialog focus and restores focus after closing', async () => {
    let remoteChanged = false;
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-stale-focus', slug: 'stale-focus', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/stale-focus/documents') {
        return json([
          {
            id: 'doc-stale-focus',
            path: 'daily.md',
            head_version_id: remoteChanged ? 'v2' : 'v1',
            content_type: 'text/markdown',
            byte_size: 8,
            metadata: { title: 'Daily' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/stale-focus/documents/daily.md' && init?.method === 'PUT') {
        remoteChanged = true;
        return new Response(JSON.stringify({ error: 'stale' }), {
          headers: { 'content-type': 'application/json', ETag: '"v2"' },
          status: 412,
        });
      }
      if (url === '/v1/libraries/stale-focus/documents/daily.md') {
        return new Response(remoteChanged ? '# Remote' : '# Base', {
          headers: { ETag: remoteChanged ? '"v2"' : '"v1"', 'content-type': 'text/markdown' },
        });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks') || url.endsWith('/versions')) {
        return json(url.endsWith('/versions') ? [] : { path: 'daily.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/stale-focus/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url === '/v1/libraries/stale-focus/conflicts') return json([]);
      if (url === '/v1/libraries/stale-focus/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/stale-focus/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Daily/ }));
    await waitFor(() => expect(screen.getByLabelText('Markdown source')).toHaveValue('# Base'));
    const editor = screen.getByLabelText('Markdown source');
    await userEvent.clear(editor);
    await userEvent.type(editor, '# Local');
    const save = screen.getByRole('button', { name: 'Save document' });
    await userEvent.click(save);

    const dialog = await screen.findByRole('dialog', { name: 'Save conflict' });
    const useRemote = within(dialog).getByRole('button', { name: 'Use remote' });
    const keepEditing = within(dialog).getByRole('button', { name: 'Keep editing local draft' });
    await waitFor(() => expect(useRemote).toHaveFocus());

    await userEvent.tab({ shift: true });
    expect(keepEditing).toHaveFocus();

    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('dialog', { name: 'Save conflict' })).not.toBeInTheDocument();
    expect(editor).toHaveFocus();
  });

  it('shows document properties for the open document', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-props', slug: 'properties-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/properties-lib/documents') {
        return json([
          {
            id: 'doc-props',
            path: 'notes/details.md',
            head_version_id: 'v-props',
            content_type: 'text/markdown',
            byte_size: 2048,
            metadata: { aliases: ['Spec Alias'], custom: 'Reviewed', title: 'Details' },
            updated_at: '2026-05-28T12:00:00Z',
          },
        ]);
      }
      if (url === '/v1/libraries/properties-lib/documents/notes/details.md') {
        return new Response('# Details', { headers: { ETag: '"v-props"', 'content-type': 'text/markdown' } });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'notes/details.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/properties-lib/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url === '/v1/libraries/properties-lib/conflicts') return json([]);
      if (url.startsWith('/v1/libraries/properties-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Details/ }));
    await userEvent.click(screen.getByRole('tab', { name: 'Properties' }));
    const properties = screen.getByRole('tabpanel', { name: 'Properties' });

    expect(within(properties).getByText('Path')).toBeInTheDocument();
    expect(within(properties).getByText('notes/details.md')).toBeInTheDocument();
    expect(within(properties).getByText('Type')).toBeInTheDocument();
    expect(within(properties).getByText('text/markdown')).toBeInTheDocument();
    expect(within(properties).getByText('Size')).toBeInTheDocument();
    expect(within(properties).getByText('2 KB')).toBeInTheDocument();
    expect(within(properties).getByText('Version')).toBeInTheDocument();
    expect(within(properties).getByText('v-props')).toBeInTheDocument();
    expect(within(properties).getByText('Updated')).toBeInTheDocument();
    expect(within(properties).getByText('2026-05-28T12:00:00Z')).toBeInTheDocument();
    expect(within(properties).getByText('Title')).toBeInTheDocument();
    expect(within(properties).getByText('Details')).toBeInTheDocument();
    expect(within(properties).getByText('Aliases')).toBeInTheDocument();
    expect(within(properties).getByText('Spec Alias')).toBeInTheDocument();
    expect(within(properties).getByText('Custom')).toBeInTheDocument();
    expect(within(properties).getByText('Reviewed')).toBeInTheDocument();
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
    expect(await screen.findByText('guide.md')).toBeInTheDocument();
    expect(screen.queryByText('source.md')).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole('tab', { name: 'Backlinks' }));
    expect(screen.getByRole('tab', { name: 'Backlinks' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByText('source.md')).toBeInTheDocument();
    expect(screen.queryByText('guide.md')).not.toBeInTheDocument();
    expect(localStorage.getItem('quarry:right-pane-tab:right-tabs')).toBe('backlinks');

    unmount();
    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Tabbed/ }));
    expect(screen.getByRole('tab', { name: 'Backlinks' })).toHaveAttribute('aria-selected', 'true');
    expect(await screen.findByText('source.md')).toBeInTheDocument();
  });

  it('loads the selected library and document from the route and updates the route on open', async () => {
    window.history.pushState({}, '', '/libraries/routed-lib/documents/folder/deep.md');
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

    await waitFor(() => expect(screen.getByLabelText('Markdown source')).toHaveValue('# Deep'));
    await userEvent.click(screen.getByRole('treeitem', { name: /Next/ }));

    await waitFor(() => expect(screen.getByLabelText('Markdown source')).toHaveValue('# Next'));
    expect(window.location.pathname).toBe('/libraries/routed-lib/documents/next.md');
  });

  it('creates and selects a new library from the library switcher', async () => {
    const prompt = vi.spyOn(window, 'prompt').mockReturnValue('research');
    let libraries = [
      { id: 'lib-personal', slug: 'personal', created_at: 'now', settings: {} },
      { id: 'lib-work', slug: 'work', created_at: 'now', settings: {} },
    ];
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/v1/libraries' && init?.method === 'POST') {
        expect(JSON.parse(String(init.body))).toMatchObject({ slug: 'research' });
        const library = { id: 'lib-research', slug: 'research', created_at: 'now', settings: {} };
        libraries = [...libraries, library];
        return json(library);
      }
      if (url === '/v1/libraries') {
        return json(libraries);
      }
      if (url === '/v1/libraries/research/documents') return json([]);
      if (url === '/v1/libraries/research/conflicts') return json([]);
      if (url === '/v1/libraries/research/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/research/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.startsWith('/v1/libraries/research/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    expect(await screen.findByRole('combobox')).toHaveValue('');
    await userEvent.click(screen.getByRole('button', { name: 'Create library' }));

    expect(prompt).toHaveBeenCalledWith('New library slug');
    await waitFor(() => expect(screen.getByRole('combobox')).toHaveValue('research'));
    expect(localStorage.getItem('quarry:active-library')).toBe('research');
    expect(window.location.pathname).toBe('/libraries/research');
  });

  it('orders the library switcher by recent Libraries and updates recency on selection', async () => {
    localStorage.setItem('quarry:recent-libraries', JSON.stringify(['work']));
    const libraries = [
      { id: 'lib-personal', slug: 'personal', created_at: 'now', settings: {} },
      { id: 'lib-work', slug: 'work', created_at: 'now', settings: {} },
      { id: 'lib-archive', slug: 'archive', created_at: 'now', settings: {} },
    ];
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') return json(libraries);
      if (url === '/v1/libraries/archive/documents') return json([]);
      if (url === '/v1/libraries/archive/conflicts') return json([]);
      if (url === '/v1/libraries/archive/git/peers') return json([]);
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    const switcher = (await screen.findByRole('combobox', {
      name: 'Library switcher',
    })) as HTMLSelectElement;
    expect(Array.from(switcher.options).map((option) => option.value)).toEqual([
      '',
      'work',
      'personal',
      'archive',
    ]);

    await userEvent.selectOptions(switcher, 'archive');

    await waitFor(() => expect(localStorage.getItem('quarry:active-library')).toBe('archive'));
    expect(JSON.parse(localStorage.getItem('quarry:recent-libraries') ?? '[]')).toEqual([
      'archive',
      'work',
    ]);
    expect(
      Array.from((screen.getByRole('combobox', { name: 'Library switcher' }) as HTMLSelectElement).options).map(
        (option) => option.value
      )
    ).toEqual(['', 'archive', 'work', 'personal']);
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

    fireEvent.contextMenu(treeItemLabel(/Source/, 'Source'), { clientX: 40, clientY: 72 });
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Reveal in graph' }));
    await waitFor(() => expect(screen.getByLabelText('Markdown source')).toHaveValue('# Source'));
    expect(window.location.pathname).toBe('/libraries/tree-lib/documents/folder/source.md');

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

    expect(await screen.findAllByText('guide.md')).toHaveLength(2);
    expect(screen.getByText('Missing')).toBeInTheDocument();
    expect(screen.getByText('Unresolved')).toBeInTheDocument();
    expect(screen.getByText('Duplicate')).toBeInTheDocument();
    expect(screen.getByText('Ambiguous')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Create document for Duplicate' })).not.toBeInTheDocument();
    expect(screen.getByLabelText('Markdown source')).toHaveValue(
      '# Links\n\nSee [[Guide]], [[Missing]], and [Guide link](guide.md).'
    );
    await userEvent.click(screen.getByRole('button', { name: 'Rich' }));
    await userEvent.click(await screen.findByRole('button', { name: 'Guide link' }));
    await waitFor(() =>
      expect(screen.getByLabelText('Plate markdown editor')).toHaveTextContent('Hover preview body.')
    );
    await userEvent.click(await screen.findByRole('treeitem', { name: /Links/ }));
    const resolvedLinkButtons = screen.getAllByRole('button', { name: 'guide.md' });
    await userEvent.hover(resolvedLinkButtons[0]);
    expect((await screen.findAllByLabelText('Link preview'))[0]).toHaveTextContent('Hover preview body.');
    await userEvent.unhover(resolvedLinkButtons[0]);
    await waitFor(() => expect(screen.queryAllByLabelText('Link preview')).toHaveLength(0));
    await userEvent.click(screen.getByRole('tab', { name: 'Backlinks' }));
    expect(screen.getByText('source.md')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('tab', { name: 'Graph' }));
    expect(await screen.findByText('links.md -> guide.md')).toBeInTheDocument();

    await userEvent.click(screen.getByRole('tab', { name: 'Links' }));
    await userEvent.click(screen.getByRole('button', { name: 'Create document for Missing' }));
    expect(prompt).toHaveBeenCalledWith('New document path', 'Missing.md');
    await waitFor(() => expect(screen.getByLabelText('Plate markdown editor')).toHaveTextContent('Untitled'));

    await userEvent.click(screen.getAllByRole('button', { name: 'guide.md' })[0]);
    await waitFor(() =>
      expect(screen.getByLabelText('Plate markdown editor')).toHaveTextContent('Hover preview body.')
    );
  });

  it('switches between focused and full-library graph modes and persists the choice per library', async () => {
    const graphRequests: string[] = [];
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-graph-mode', slug: 'graph-mode', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/graph-mode/documents') {
        return json([
          {
            id: 'doc-home',
            path: 'home.md',
            head_version_id: 'v-home',
            content_type: 'text/markdown',
            byte_size: 6,
            metadata: { title: 'Home' },
            updated_at: 'now',
          },
          {
            id: 'doc-away',
            path: 'away.md',
            head_version_id: 'v-away',
            content_type: 'text/markdown',
            byte_size: 6,
            metadata: { title: 'Away' },
            updated_at: 'now',
          },
          {
            id: 'doc-deep',
            path: 'deep.md',
            head_version_id: 'v-deep',
            content_type: 'text/markdown',
            byte_size: 6,
            metadata: { title: 'Deep' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/graph-mode/documents/home.md') {
        return new Response('# Home', { headers: { ETag: '"v-home"', 'content-type': 'text/markdown' } });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'home.md', links: [] });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url.startsWith('/v1/libraries/graph-mode/graph')) {
        graphRequests.push(url);
        if (url.includes('tag=planning')) {
          return json({
            nodes: [{ id: 'doc-brief', path: 'projects/brief.md', title: 'Brief', content_type: 'text/markdown' }],
            edges: [
              {
                id: 'edge-filter-tag',
                source: 'doc-brief',
                source_path: 'projects/brief.md',
                target: null,
                target_path: null,
                target_kind: 'tag',
                target_text: 'planning',
                resolved: false,
              },
            ],
            truncated: false,
          });
        }
        if (url.includes('folder=projects')) {
          return json({
            nodes: [
              { id: 'doc-brief', path: 'projects/brief.md', title: 'Brief', content_type: 'text/markdown' },
              {
                id: 'doc-roadmap',
                path: 'projects/roadmap.md',
                title: 'Roadmap',
                content_type: 'text/markdown',
              },
            ],
            edges: [
              {
                id: 'edge-folder',
                source: 'doc-brief',
                source_path: 'projects/brief.md',
                target: 'doc-roadmap',
                target_path: 'projects/roadmap.md',
                target_kind: 'wiki_link',
                target_text: 'Roadmap',
                resolved: true,
              },
            ],
            truncated: false,
          });
        }
        if (url.includes('link_kind=tag')) {
          return json({
            nodes: [{ id: 'doc-home', path: 'home.md', title: 'Home', content_type: 'text/markdown' }],
            edges: [
              {
                id: 'edge-tag',
                source: 'doc-home',
                source_path: 'home.md',
                target: null,
                target_path: null,
                target_kind: 'tag',
                target_text: 'planning',
                resolved: true,
              },
            ],
            truncated: false,
          });
        }
        if (url.includes('resolved=false')) {
          return json({
            nodes: [{ id: 'doc-home', path: 'home.md', title: 'Home', content_type: 'text/markdown' }],
            edges: [
              {
                id: 'edge-unresolved',
                source: 'doc-home',
                source_path: 'home.md',
                target: null,
                target_path: null,
                target_kind: 'wiki_link',
                target_text: 'Missing',
                resolved: false,
              },
            ],
            truncated: false,
          });
        }
        if (url.includes('root=home.md') && url.includes('depth=2')) {
          return json({
            nodes: [
              { id: 'doc-home', path: 'home.md', title: 'Home', content_type: 'text/markdown' },
              { id: 'doc-away', path: 'away.md', title: 'Away', content_type: 'text/markdown' },
              { id: 'doc-deep', path: 'deep.md', title: 'Deep', content_type: 'text/markdown' },
            ],
            edges: [
              {
                id: 'edge-depth',
                source: 'doc-away',
                source_path: 'away.md',
                target: 'doc-deep',
                target_path: 'deep.md',
                target_kind: 'wiki_link',
                target_text: 'Deep',
                resolved: true,
              },
            ],
            truncated: false,
          });
        }
        if (url.includes('root=home.md')) {
          return json({
            nodes: [{ id: 'doc-home', path: 'home.md', title: 'Home', content_type: 'text/markdown' }],
            edges: [],
            truncated: false,
          });
        }
        return json({
          nodes: [
            { id: 'doc-home', path: 'home.md', title: 'Home', content_type: 'text/markdown' },
            { id: 'doc-away', path: 'away.md', title: 'Away', content_type: 'text/markdown' },
            { id: 'doc-deep', path: 'deep.md', title: 'Deep', content_type: 'text/markdown' },
          ],
          edges: [
            {
              id: 'edge-full',
              source: 'doc-away',
              source_path: 'away.md',
              target: 'doc-home',
              target_path: 'home.md',
              target_kind: 'wiki_link',
              target_text: 'Home',
              resolved: true,
            },
          ],
          truncated: true,
        });
      }
      if (url === '/v1/libraries/graph-mode/conflicts') return json([]);
      if (url.startsWith('/v1/libraries/graph-mode/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    const { unmount } = renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Home/ }));
    await userEvent.click(screen.getByRole('tab', { name: 'Graph' }));
    expect(screen.getByRole('button', { name: 'Focused' })).toHaveAttribute('aria-pressed', 'true');
    await waitFor(() => expect(graphRequests).toContain('/v1/libraries/graph-mode/graph?root=home.md'));
    expect(screen.queryByText('away.md -> home.md')).not.toBeInTheDocument();

    await userEvent.selectOptions(screen.getByLabelText('Graph depth'), '2');
    await waitFor(() => expect(graphRequests).toContain('/v1/libraries/graph-mode/graph?root=home.md&depth=2'));
    expect(await screen.findByText('away.md -> deep.md')).toBeInTheDocument();
    expect(localStorage.getItem('quarry:graph-depth:graph-mode')).toBe('2');

    await userEvent.selectOptions(screen.getByLabelText('Graph link kind'), 'tag');
    await waitFor(() =>
      expect(graphRequests).toContain('/v1/libraries/graph-mode/graph?root=home.md&depth=2&link_kind=tag')
    );
    expect(await screen.findByText('home.md -> planning')).toBeInTheDocument();
    expect(localStorage.getItem('quarry:graph-link-kind:graph-mode')).toBe('tag');

    await userEvent.selectOptions(screen.getByLabelText('Graph link kind'), 'all');
    await userEvent.selectOptions(screen.getByLabelText('Graph resolution'), 'unresolved');
    await waitFor(() =>
      expect(graphRequests).toContain('/v1/libraries/graph-mode/graph?root=home.md&depth=2&resolved=false')
    );
    expect(await screen.findByText('home.md -> Missing')).toBeInTheDocument();
    expect(localStorage.getItem('quarry:graph-resolution:graph-mode')).toBe('unresolved');

    await userEvent.selectOptions(screen.getByLabelText('Graph resolution'), 'all');
    await userEvent.click(screen.getByRole('button', { name: 'Full library' }));
    expect(screen.getByRole('button', { name: 'Full library' })).toHaveAttribute('aria-pressed', 'true');
    await waitFor(() => expect(graphRequests).toContain('/v1/libraries/graph-mode/graph'));
    expect(await screen.findByText('away.md -> home.md')).toBeInTheDocument();
    expect(screen.getByText(/Full graph is too large/)).toBeInTheDocument();
    expect(screen.getByText(/use focused mode or filters/i)).toBeInTheDocument();
    expect(localStorage.getItem('quarry:graph-scope:graph-mode')).toBe('full');

    fireEvent.change(screen.getByLabelText('Graph folder'), { target: { value: 'projects' } });
    expect(screen.getByLabelText('Graph folder')).toHaveValue('projects');
    expect(localStorage.getItem('quarry:graph-folder:graph-mode')).toBe('projects');
    await waitFor(() => expect(graphRequests.some((url) => url.includes('folder=projects'))).toBe(true));
    expect(await screen.findByText('projects/brief.md -> projects/roadmap.md')).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText('Graph tag'), { target: { value: 'planning' } });
    await waitFor(() =>
      expect(graphRequests.some((url) => url.includes('folder=projects') && url.includes('tag=planning'))).toBe(true)
    );
    expect(await screen.findByText('projects/brief.md -> planning')).toBeInTheDocument();
    expect(localStorage.getItem('quarry:graph-tag:graph-mode')).toBe('planning');

    unmount();
    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Home/ }));
    await userEvent.click(screen.getByRole('tab', { name: 'Graph' }));
    expect(await screen.findByRole('button', { name: 'Full library' })).toHaveAttribute('aria-pressed', 'true');
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
        return json([version(restored ? 'v3' : 'v2'), version('v1')]);
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
    await waitFor(() => expect(screen.getByLabelText('Markdown source')).toHaveValue('# Old'));
    expect(fetch).toHaveBeenCalledWith(
      '/v1/libraries/versions-lib/documents/versioned.md/versions/v1/restore',
      expect.objectContaining({ method: 'POST' })
    );
  });

  it('shows version metadata in the version history list', async () => {
    const historicalVersion = {
      id: 'v-meta',
      document_id: 'doc-version-meta',
      tx_id: 'tx-import-1',
      content_hash: null,
      inline_content: null,
      metadata: {},
      transaction_actor: 'Avery',
      transaction_source: 'git',
      transaction_message: 'Imported from Git',
      transaction_provenance: { remote: 'origin/main' },
      content_type: 'text/markdown',
      byte_size: 2048,
      created_at: '2026-05-28T12:00:00Z',
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

    const transactionText = await screen.findByText(/tx-import-1/);
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
      if (url.endsWith('/versions')) return json([version('v3'), version('v2'), version('v1')]);
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

  it('diffs the current editor content against the latest server content', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-current-diff', slug: 'current-diff-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/current-diff-lib/documents') {
        return json([
          {
            id: 'doc-current-diff',
            path: 'draft.md',
            head_version_id: 'v-head',
            content_type: 'text/markdown',
            byte_size: 16,
            metadata: { title: 'Draft' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/current-diff-lib/documents/draft.md') {
        return new Response('# Server\nBody', {
          headers: { ETag: '"v-head"', 'content-type': 'text/markdown' },
        });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'draft.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/current-diff-lib/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) return json([version('v-head')]);
      if (url === '/v1/libraries/current-diff-lib/conflicts') return json([]);
      if (url.startsWith('/v1/libraries/current-diff-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Draft/ }));
    await userEvent.clear(await screen.findByLabelText('Markdown source'));
    await userEvent.type(screen.getByLabelText('Markdown source'), '# Local\nBody changed');
    await userEvent.click(screen.getByRole('tab', { name: 'Versions' }));
    await userEvent.click(screen.getByRole('button', { name: 'Diff editor against latest' }));

    expect(screen.getByText('Current editor vs latest server')).toBeInTheDocument();
    expect(screen.getByText(/-# Server/)).toBeInTheDocument();
    expect(screen.getByText(/\+# Local/)).toBeInTheDocument();
    expect(screen.getByText(/-Body/)).toBeInTheDocument();
    expect(screen.getByText(/\+Body changed/)).toBeInTheDocument();
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
    expect(await screen.findByLabelText('Markdown source')).toHaveValue('# Initial');
    expect(MockEventSource.instances[0]?.url).toBe('/v1/events?library=events-lib');

    content = '# External';
    act(() => {
      MockEventSource.instances[0].emit('doc.changed', {
        type: 'doc.changed',
        library: 'events-lib',
        path: 'daily.md',
      });
    });

    await waitFor(() => expect(screen.getByLabelText('Markdown source')).toHaveValue('# External'));

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

    await waitFor(() => expect(screen.getByLabelText('Markdown source')).toHaveValue('# Git synced'));
    expect(screen.getByText('1 documents · 1 conflicts')).toBeInTheDocument();
    expect(within(screen.getByRole('contentinfo')).getByText('Last sync: Peer peer-main · Applied 2 · Conflicts 1')).toBeInTheDocument();
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
    expect(await screen.findByLabelText('Markdown source')).toHaveValue('# Initial');

    content = '# Polled';
    outgoing = [link({ src_path: 'daily.md', target_text: 'Guide', target_path: 'guide.md' })];
    act(() => {
      MockEventSource.instances[0].onerror?.(new Event('error'));
    });

    await waitFor(() => expect(screen.getByLabelText('Markdown source')).toHaveValue('# Polled'));
    expect(screen.getByText('guide.md')).toBeInTheDocument();
    expect(within(screen.getByRole('contentinfo')).getByText('Polling')).toBeInTheDocument();
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
    await userEvent.click(screen.getByRole('tab', { name: 'Conflicts' }));
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
        expect(init.headers).toMatchObject({ 'If-Match': '"head"' });
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

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Conflict/ }));
    await userEvent.click(screen.getByRole('tab', { name: 'Conflicts' }));
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
    expect(screen.queryByLabelText('Markdown source')).not.toBeInTheDocument();

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
    expect(screen.queryByLabelText('Markdown source')).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole('treeitem', { name: /Gallery/ }));
    await waitFor(() =>
      expect(screen.getByLabelText('Markdown source')).toHaveValue('# Gallery\n\n![Project photo](assets/photo.png)')
    );
    await userEvent.click(screen.getByRole('button', { name: 'Rich' }));
    expect(await screen.findByRole('img', { name: 'Project photo' })).toHaveAttribute(
      'src',
      '/v1/libraries/media-lib/documents/assets/photo.png'
    );
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

    await waitFor(() => expect(screen.getByLabelText('Markdown source')).toHaveValue('# Guide'));
  });

  it('opens workspace settings from the top bar and command palette', async () => {
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

    await userEvent.click(await screen.findByRole('button', { name: 'Settings' }));
    let settings = screen.getByRole('dialog', { name: 'Workspace settings' });
    expect(within(settings).getByText('settings-lib')).toBeInTheDocument();
    expect(within(settings).getByText('quarry:layout:settings-lib')).toBeInTheDocument();

    await userEvent.click(within(settings).getByRole('button', { name: 'Use dark theme' }));
    expect(screen.getByRole('main')).toHaveAttribute('data-theme', 'dark');
    expect(localStorage.getItem('quarry:theme')).toBe('dark');

    await userEvent.click(within(settings).getByRole('button', { name: 'Reset workspace layout' }));
    expect(localStorage.getItem('quarry:layout:settings-lib')).toBeNull();

    await userEvent.click(within(settings).getByRole('button', { name: 'Close settings' }));
    expect(screen.queryByRole('dialog', { name: 'Workspace settings' })).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole('button', { name: 'Commands' }));
    await userEvent.click(await screen.findByText('Open settings'));
    settings = screen.getByRole('dialog', { name: 'Workspace settings' });
    expect(settings).toBeInTheDocument();
    expect(within(settings).queryByText('Not configured')).not.toBeInTheDocument();
  });

  it('traps settings dialog focus and restores it to the launcher', async () => {
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

    const launcher = await screen.findByRole('button', { name: 'Settings' });
    launcher.focus();
    await userEvent.keyboard('{Enter}');
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
    expect(launcher).toHaveFocus();
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

    await userEvent.clear(await screen.findByPlaceholderText('Search'));
    await userEvent.type(screen.getByPlaceholderText('Search'), 'guide');
    const results = await screen.findByRole('listbox', { name: 'Search results' });
    expect(screen.getByLabelText('Search result preview')).toHaveTextContent('Guide snippet');

    results.focus();
    await userEvent.keyboard('{ArrowDown}');
    expect(screen.getByLabelText('Search result preview')).toHaveTextContent('Journal snippet');

    await userEvent.keyboard('{Enter}');

    await waitFor(() => expect(screen.getByLabelText('Markdown source')).toHaveValue('# Journal'));
    expect(window.location.pathname).toBe('/libraries/search-key-lib/documents/journal.md');
  });

  it('asks before replacing an unsaved draft from a search result click', async () => {
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-search', slug: 'search-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/search-lib/documents') {
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
      if (url === '/v1/libraries/search-lib/documents/daily.md') {
        return new Response('# Daily', { headers: { ETag: '"v-daily"', 'content-type': 'text/markdown' } });
      }
      if (url === '/v1/libraries/search-lib/documents/guide.md') {
        return new Response('# Guide', { headers: { ETag: '"v-guide"', 'content-type': 'text/markdown' } });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'document.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/search-lib/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url === '/v1/libraries/search-lib/conflicts') return json([]);
      if (url === '/v1/libraries/search-lib/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/search-lib/search')) {
        return json({
          results: [
            {
              document_id: 'doc-guide',
              path: 'guide.md',
              title: 'Guide',
              content_type: 'text/markdown',
              score: 1,
              snippet: '# Guide',
              matched_fields: ['title'],
              head_version_id: 'v-guide',
            },
          ],
          cursor: null,
        });
      }
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Daily/ }));
    await waitFor(() => expect(screen.getByLabelText('Markdown source')).toHaveValue('# Daily'));
    const editor = screen.getByLabelText('Markdown source') as HTMLTextAreaElement;
    await userEvent.clear(editor);
    await userEvent.type(editor, '# Daily draft');
    await waitFor(() => expect(screen.getByText('Draft saved locally')).toBeInTheDocument());
    await userEvent.clear(screen.getByPlaceholderText('Search'));
    await userEvent.type(screen.getByPlaceholderText('Search'), 'guide');
    await waitFor(() =>
      expect(fetch).toHaveBeenCalledWith('/v1/libraries/search-lib/search?q=guide&limit=50', undefined)
    );
    await userEvent.click((await screen.findAllByText('Guide'))[0]);

    expect(confirm).toHaveBeenCalledWith('Open guide.md and keep your unsaved draft for daily.md?');
    expect(screen.getByLabelText('Markdown source')).toHaveValue('# Daily draft');
    expect(window.location.pathname).toBe('/libraries/search-lib/documents/daily.md');
  });

  it('uses server wiki suggestions to complete source wiki links', async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-wiki-suggest', slug: 'wiki-suggest-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/wiki-suggest-lib/documents') {
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
      if (url === '/v1/libraries/wiki-suggest-lib/documents/daily.md') {
        return new Response('# Daily', { headers: { ETag: '"v-daily"', 'content-type': 'text/markdown' } });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'document.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/wiki-suggest-lib/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url === '/v1/libraries/wiki-suggest-lib/conflicts') return json([]);
      if (url === '/v1/libraries/wiki-suggest-lib/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/wiki-suggest-lib/search/suggest')) {
        return json([
          {
            path: 'guide.md',
            title: 'Guide',
            match_type: 'title',
            head_version_id: 'v-guide',
            matched_text: 'Guide',
            target_anchor: null,
          },
        ]);
      }
      if (url.startsWith('/v1/libraries/wiki-suggest-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Daily/ }));
    const editor = await screen.findByLabelText('Markdown source');
    await waitFor(() => expect(editor).toHaveValue('# Daily'));
    await userEvent.clear(editor);
    // user-event escapes a literal "[" as "[[".
    await userEvent.type(editor, 'See [[[[gui');
    expect(editor).toHaveValue('See [[gui');
    await waitFor(() =>
      expect(fetch).toHaveBeenCalledWith('/v1/libraries/wiki-suggest-lib/search/suggest?q=gui&limit=20', undefined)
    );
    await userEvent.click(await screen.findByRole('option', { name: /Guide/ }));

    expect(screen.getByLabelText('Markdown source')).toHaveValue('See [[guide]]');
  });

  it('keeps oversized markdown in source mode unless rich editing is confirmed', async () => {
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === '/v1/libraries') {
        return json([{ id: 'lib-large', slug: 'large-lib', created_at: 'now', settings: {} }]);
      }
      if (url === '/v1/libraries/large-lib/documents') {
        return json([
          {
            id: 'doc-small',
            path: 'small.md',
            head_version_id: 'v-small',
            content_type: 'text/markdown',
            byte_size: 128,
            metadata: { title: 'Small' },
            updated_at: 'now',
          },
          {
            id: 'doc-big',
            path: 'big.md',
            head_version_id: 'v-big',
            content_type: 'text/markdown',
            byte_size: 2 * 1024 * 1024 + 1,
            metadata: { title: 'Big' },
            updated_at: 'now',
          },
          {
            id: 'doc-medium',
            path: 'medium.md',
            head_version_id: 'v-medium',
            content_type: 'text/markdown',
            byte_size: 512 * 1024 + 1,
            metadata: { title: 'Medium' },
            updated_at: 'now',
          },
        ]);
      }
      if (url === '/v1/libraries/large-lib/documents/small.md') {
        return new Response('# Small', { headers: { ETag: '"v-small"', 'content-type': 'text/markdown' } });
      }
      if (url === '/v1/libraries/large-lib/documents/big.md') {
        return new Response('# Big', { headers: { ETag: '"v-big"', 'content-type': 'text/markdown' } });
      }
      if (url === '/v1/libraries/large-lib/documents/medium.md') {
        return new Response('# Medium', { headers: { ETag: '"v-medium"', 'content-type': 'text/markdown' } });
      }
      if (url.endsWith('/outgoing-links') || url.endsWith('/backlinks')) {
        return json({ path: 'document.md', links: [] });
      }
      if (url.startsWith('/v1/libraries/large-lib/graph')) {
        return json({ nodes: [], edges: [], truncated: false });
      }
      if (url.endsWith('/versions')) return json([]);
      if (url === '/v1/libraries/large-lib/conflicts') return json([]);
      if (url === '/v1/libraries/large-lib/git/peers') return json([]);
      if (url.startsWith('/v1/libraries/large-lib/search')) return json({ results: [], cursor: null });
      return new Response('not found', { status: 404 });
    });
    vi.stubGlobal('fetch', fetch);

    renderApp();

    await userEvent.click(await screen.findByRole('treeitem', { name: /Small/ }));
    await waitFor(() => expect(screen.getByLabelText('Markdown source')).toHaveValue('# Small'));
    await userEvent.click(screen.getByRole('button', { name: 'Rich' }));
    expect(screen.getByRole('button', { name: 'Rich' })).toHaveAttribute('aria-pressed', 'true');
    expect(await screen.findByLabelText('Rich markdown preview')).toBeInTheDocument();

    await userEvent.click(screen.getByRole('treeitem', { name: /Medium/ }));
    await waitFor(() => expect(screen.getByLabelText('Plate markdown editor')).toHaveTextContent('Medium'));
    expect(screen.getByRole('button', { name: 'Rich' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.queryByLabelText('Rich markdown preview')).not.toBeInTheDocument();
    expect(confirm).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole('treeitem', { name: /Big/ }));
    await waitFor(() => expect(screen.getByLabelText('Markdown source')).toHaveValue('# Big'));
    expect(screen.getByRole('button', { name: 'Source' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.queryByLabelText('Rich markdown preview')).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole('button', { name: 'Rich' }));

    expect(confirm).toHaveBeenCalledWith('Open rich editing for big.md? This document is over 2 MiB and may be slow.');
    expect(screen.getByRole('button', { name: 'Source' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.queryByLabelText('Rich markdown preview')).not.toBeInTheDocument();
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

    await userEvent.click(await screen.findByRole('button', { name: 'Sync' }));
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
    expect(
      within(screen.getByRole('contentinfo')).getByText('Last sync: Imported 1 · Exported 2 · Conflicts 1')
    ).toBeInTheDocument();
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
    await userEvent.click(screen.getByRole('button', { name: 'Toggle dark mode' }));
    expect(screen.getByRole('main')).toHaveAttribute('data-theme', 'dark');
    expect(localStorage.getItem('quarry:theme')).toBe('dark');

    unmount();
    renderApp();

    expect(await screen.findByRole('main')).toHaveAttribute('data-theme', 'dark');
  });
});

function renderApp() {
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
