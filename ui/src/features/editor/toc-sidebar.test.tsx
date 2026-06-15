import { render, screen, within } from '@testing-library/react';

import { MarkdownEditor } from './MarkdownEditor';

describe('TocSidebar', () => {
  it('lists every heading and stamps a DOM id on each rendered heading', async () => {
    const { container } = render(
      <MarkdownEditor
        content={'# Introduction\n\n## Details\n\nbody text'}
        mode="editing"
        onChange={() => {}}
      />
    );

    await screen.findByLabelText('Plate markdown editor');

    // Headings carry a DOM id so the scroll-spy can read entry.target.id.
    const h1 = container.querySelector('.slate-h1');
    const h2 = container.querySelector('.slate-h2');
    expect(h1?.id).toBeTruthy();
    expect(h2?.id).toBeTruthy();

    // One TOC entry per heading.
    const nav = screen.getByLabelText('Table of contents');
    const items = within(nav).getAllByRole('button');
    expect(items).toHaveLength(2);
    expect(within(nav).getByText('Introduction')).toBeInTheDocument();
    expect(within(nav).getByText('Details')).toBeInTheDocument();
  });

  it('renders nothing with fewer than two headings', async () => {
    render(
      <MarkdownEditor content={'# Lonely\n\nbody'} mode="editing" onChange={() => {}} />
    );

    await screen.findByLabelText('Plate markdown editor');

    expect(screen.queryByLabelText('Table of contents')).not.toBeInTheDocument();
  });
});
