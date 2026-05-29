import { render, screen } from '@testing-library/react';

import { MarkdownEditor } from './MarkdownEditor';

describe('MarkdownEditor', () => {
  it('uses a Plate editor as the editing surface', async () => {
    render(<MarkdownEditor content="# Guide" status="Saved" onChange={() => {}} />);

    const editor = await screen.findByLabelText('Plate markdown editor');
    expect(editor).toHaveAttribute('contenteditable', 'true');
    expect(editor).toHaveAttribute('data-slate-editor', 'true');
  });

  it('offers a limited formatting toolbar', async () => {
    render(<MarkdownEditor content="# Guide" status="Saved" onChange={() => {}} />);

    await screen.findByLabelText('Plate markdown editor');
    expect(screen.getByRole('button', { name: 'Bold' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Italic' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Strikethrough' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Inline code' })).toBeInTheDocument();
  });
});
