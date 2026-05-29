import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import { MarkdownEditor } from './MarkdownEditor';

const mermaidMock = {
  initialize: vi.fn(),
  render: vi.fn(async () => ({
    svg: '<svg><text>Rendered graph</text><script>window.__bad = true</script></svg>',
  })),
};

describe('MarkdownEditor rich preview', () => {
  beforeEach(() => {
    mermaidMock.initialize.mockClear();
    mermaidMock.render.mockClear();
  });

  it('renders wiki links and Mermaid fences without executing raw HTML', async () => {
    const openDocument = vi.fn();
    render(
      <MarkdownEditor
        content={[
          'Link to [[Guide|the guide]], [[Missing]], [[Duplicated|duplicate]], and ![[Diagram]].',
          '',
          '```mermaid',
          'graph TD',
          '  A --> B',
          '```',
          '',
          '<img src=x onerror=alert(1)>',
        ].join('\n')}
        mode="rich"
        loadMermaid={async () => mermaidMock}
        links={[
          {
            target_kind: 'wiki_link',
            target_text: 'Guide',
            target_path: 'guide.md',
            resolved: true,
          },
          {
            target_kind: 'wiki_link',
            target_text: 'Missing',
            target_path: null,
            resolved: false,
          },
          {
            target_kind: 'wiki_link',
            target_text: 'Duplicated',
            target_path: null,
            resolution_status: 'ambiguous',
            resolved: false,
          },
          {
            target_kind: 'embed',
            target_text: 'Diagram',
            target_path: 'diagram.md',
            resolved: true,
          },
        ]}
        status="Clean"
        onChange={() => {}}
        onModeChange={() => {}}
        onOpenDocument={openDocument}
        onSave={() => {}}
      />
    );

    const preview = screen.getByLabelText('Rich markdown preview');
    await userEvent.click(within(preview).getByRole('button', { name: 'the guide' }));
    expect(openDocument).toHaveBeenCalledWith('guide.md');
    expect(within(preview).getByText('Missing')).toBeInTheDocument();
    expect(within(preview).getByText('Unresolved')).toBeInTheDocument();
    expect(within(preview).getByText('Ambiguous')).toBeInTheDocument();
    expect(within(preview).getByText('Diagram')).toBeInTheDocument();
    expect(await within(preview).findByRole('img', { name: 'Mermaid diagram' })).toHaveTextContent('Rendered graph');
    expect(mermaidMock.initialize).toHaveBeenCalledWith(
      expect.objectContaining({ securityLevel: 'strict', startOnLoad: false })
    );
    expect(mermaidMock.render).toHaveBeenCalledWith(expect.any(String), 'graph TD\n  A --> B');
    expect(within(preview).getByText('<img src=x onerror=alert(1)>')).toBeInTheDocument();
    expect(document.querySelector('img[src="x"]')).not.toBeInTheDocument();
    await waitFor(() => expect(document.querySelector('script')).not.toBeInTheDocument());
  });

  it('uses a Plate editor as the rich editing surface', async () => {
    render(
      <MarkdownEditor
        content="# Guide"
        mode="rich"
        status="Clean"
        onChange={() => {}}
        onModeChange={() => {}}
        onSave={() => {}}
      />
    );

    const editor = await screen.findByLabelText('Plate markdown editor');
    expect(editor).toHaveAttribute('contenteditable', 'true');
    expect(editor).toHaveAttribute('data-slate-editor', 'true');
  });

  it('renders markdown links as navigable rich-preview elements', async () => {
    const openDocument = vi.fn();
    render(
      <MarkdownEditor
        content="Read [Guide](guide.md) or [external](https://example.com)."
        mode="rich"
        links={[
          {
            target_kind: 'markdown_link',
            target_text: 'guide.md',
            target_path: 'guide.md',
            resolved: true,
          },
          {
            target_kind: 'markdown_link',
            target_text: 'https://example.com',
            target_path: null,
            resolved: false,
          },
        ]}
        status="Clean"
        onChange={() => {}}
        onModeChange={() => {}}
        onOpenDocument={openDocument}
        onSave={() => {}}
      />
    );

    const preview = screen.getByLabelText('Rich markdown preview');
    await userEvent.click(within(preview).getByRole('button', { name: 'Guide' }));
    expect(openDocument).toHaveBeenCalledWith('guide.md');
    expect(within(preview).getByRole('link', { name: 'external' })).toHaveAttribute(
      'href',
      'https://example.com'
    );
  });

  it('renders resolved wiki block references as navigable rich-preview elements', async () => {
    const openDocument = vi.fn();
    render(
      <MarkdownEditor
        content="Jump to [[Guide^block-1]]."
        mode="rich"
        links={[
          {
            target_kind: 'wiki_link',
            target_text: 'Guide',
            target_path: 'guide.md',
            target_anchor: '^block-1',
            resolved: true,
          },
        ]}
        status="Clean"
        onChange={() => {}}
        onModeChange={() => {}}
        onOpenDocument={openDocument}
        onSave={() => {}}
      />
    );

    const preview = screen.getByLabelText('Rich markdown preview');
    await userEvent.click(within(preview).getByRole('button', { name: 'Guide^block-1' }));
    expect(openDocument).toHaveBeenCalledWith('guide.md');
    expect(within(preview).queryByText('Unresolved')).not.toBeInTheDocument();
  });

  it('renders resolved markdown images inline in the rich preview', async () => {
    render(
      <MarkdownEditor
        content="Logo: ![Project logo](assets/logo.png)"
        mode="rich"
        links={[
          {
            target_kind: 'markdown_link',
            target_text: 'assets/logo.png',
            target_path: 'assets/logo.png',
            resolved: true,
          },
        ]}
        resolveDocumentHref={(path) => `/v1/libraries/notes/documents/${path}`}
        status="Clean"
        onChange={() => {}}
        onModeChange={() => {}}
        onSave={() => {}}
      />
    );

    const preview = screen.getByLabelText('Rich markdown preview');
    expect(within(preview).getByRole('img', { name: 'Project logo' })).toHaveAttribute(
      'src',
      '/v1/libraries/notes/documents/assets/logo.png'
    );
  });

  it('renders headings and non-Mermaid code fences semantically', () => {
    render(
      <MarkdownEditor
        content={['# Guide', '', '```ts', 'const answer = 42;', '```'].join('\n')}
        mode="rich"
        status="Clean"
        onChange={() => {}}
        onModeChange={() => {}}
        onSave={() => {}}
      />
    );

    const preview = screen.getByLabelText('Rich markdown preview');
    expect(within(preview).getByRole('heading', { level: 1, name: 'Guide' })).toBeInTheDocument();
    const code = within(preview).getByText('const answer = 42;');
    expect(code.closest('pre')).toBeInTheDocument();
    expect(within(preview).queryByText('```ts')).not.toBeInTheDocument();
  });

  it('renders top-level frontmatter as document metadata in rich preview', () => {
    render(
      <MarkdownEditor
        content={[
          '---',
          'title: Guide',
          'aliases:',
          '  - Shortcut',
          '  - Reference',
          'status: draft',
          '---',
          '',
          '# Guide',
        ].join('\n')}
        mode="rich"
        status="Clean"
        onChange={() => {}}
        onModeChange={() => {}}
        onSave={() => {}}
      />
    );

    const preview = screen.getByLabelText('Rich markdown preview');
    const frontmatter = within(preview).getByLabelText('Frontmatter');
    expect(frontmatter).toHaveTextContent('Title');
    expect(frontmatter).toHaveTextContent('Guide');
    expect(frontmatter).toHaveTextContent('Aliases');
    expect(frontmatter).toHaveTextContent('Shortcut, Reference');
    expect(frontmatter).toHaveTextContent('Status');
    expect(frontmatter).toHaveTextContent('draft');
    expect(within(preview).getByRole('heading', { level: 1, name: 'Guide' })).toBeInTheDocument();
    expect(within(preview).queryByText('title: Guide')).not.toBeInTheDocument();
  });

  it('renders blockquotes and lists semantically', () => {
    render(
      <MarkdownEditor
        content={[
          '> Remember this.',
          '> Keep context.',
          '',
          '- Alpha',
          '- Beta',
          '',
          '1. Draft',
          '2. Ship',
        ].join('\n')}
        mode="rich"
        status="Clean"
        onChange={() => {}}
        onModeChange={() => {}}
        onSave={() => {}}
      />
    );

    const preview = screen.getByLabelText('Rich markdown preview');
    const quote = within(preview).getByText('Remember this.').closest('blockquote');
    expect(quote).toHaveTextContent('Keep context.');
    const lists = within(preview).getAllByRole('list');
    expect(lists[0].tagName).toBe('UL');
    expect(lists[1].tagName).toBe('OL');
    expect(within(preview).getByText('Alpha').closest('li')).toBeInTheDocument();
    expect(within(preview).getByText('Draft').closest('li')).toBeInTheDocument();
    expect(within(preview).queryByText('- Alpha')).not.toBeInTheDocument();
    expect(within(preview).queryByText('1. Draft')).not.toBeInTheDocument();
  });

  it('renders inline emphasis, strong, and code marks semantically', () => {
    render(
      <MarkdownEditor
        content="Use **bold**, *italic*, and `literal` text."
        mode="rich"
        status="Clean"
        onChange={() => {}}
        onModeChange={() => {}}
        onSave={() => {}}
      />
    );

    const preview = screen.getByLabelText('Rich markdown preview');
    expect(within(preview).getByText('bold').closest('strong')).toBeInTheDocument();
    expect(within(preview).getByText('italic').closest('em')).toBeInTheDocument();
    expect(within(preview).getByText('literal').closest('code')).toBeInTheDocument();
    expect(within(preview).queryByText('**bold**')).not.toBeInTheDocument();
    expect(within(preview).queryByText('*italic*')).not.toBeInTheDocument();
    expect(within(preview).queryByText('`literal`')).not.toBeInTheDocument();
  });

  it('renders inline tags as distinct rich-preview elements', () => {
    render(
      <MarkdownEditor
        content="Plan #planning with #team/frontend."
        mode="rich"
        status="Clean"
        onChange={() => {}}
        onModeChange={() => {}}
        onSave={() => {}}
      />
    );

    const preview = screen.getByLabelText('Rich markdown preview');
    expect(within(preview).getByLabelText('Tag planning')).toHaveTextContent('#planning');
    expect(within(preview).getByLabelText('Tag team/frontend')).toHaveTextContent('#team/frontend');
  });

  it('applies wiki-link autocomplete suggestions into source markdown', async () => {
    const change = vi.fn();
    render(
      <MarkdownEditor
        content="See [[gui"
        mode="source"
        status="Draft saved locally"
        wikiSuggestions={[
          {
            path: 'guide.md',
            title: 'Guide',
            match_type: 'title',
            head_version_id: 'v-guide',
            matched_text: 'Guide',
            target_anchor: null,
          },
        ]}
        onChange={change}
        onModeChange={() => {}}
        onSave={() => {}}
      />
    );

    const editor = screen.getByLabelText('Markdown source');
    await userEvent.click(editor);
    await userEvent.keyboard('{End}');
    await userEvent.click(await screen.findByRole('option', { name: /Guide/ }));

    expect(change).toHaveBeenLastCalledWith('See [[guide]]');
  });
});
