import { render, screen } from '@testing-library/react';

import { MarkdownEditor } from './MarkdownEditor';

describe('MarkdownEditor', () => {
  it('uses a Plate editor as the editing surface', async () => {
    render(<MarkdownEditor content="# Guide" mode="editing" onChange={() => {}} />);

    // The lazy editor chunk takes ~1s to evaluate under full-suite worker
    // contention; the default 1s findBy timeout is too tight.
    const editor = await screen.findByLabelText('Plate markdown editor', undefined, {
      timeout: 5000,
    });
    expect(editor).toHaveAttribute('contenteditable', 'true');
    expect(editor).toHaveAttribute('data-slate-editor', 'true');
  });

  it('shows no formatting toolbar until text is selected', async () => {
    render(<MarkdownEditor content="# Guide" mode="editing" onChange={() => {}} />);

    await screen.findByLabelText('Plate markdown editor');
    expect(screen.queryByRole('button', { name: 'Bold' })).not.toBeInTheDocument();
  });
});
