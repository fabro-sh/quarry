import { render, screen } from '@testing-library/react';
import { ParagraphPlugin, Plate, PlateContent, usePlateEditor } from 'platejs/react';
import { describe, expect, it } from 'vitest';

import { RawMarkdownPlugin, rawMarkdownMdRules, RAW_MARKDOWN_KEY } from './raw-markdown';

function Harness() {
  const editor = usePlateEditor({
    plugins: [ParagraphPlugin, RawMarkdownPlugin],
    value: [
      { type: 'p', children: [{ text: 'Before.' }] },
      {
        type: RAW_MARKDOWN_KEY,
        markdown: 'See [[guide|the guide]] and ![icon](a.png) inline.',
        children: [{ text: '' }],
      },
      { type: 'p', children: [{ text: 'After.' }] },
    ],
  });
  return (
    <Plate editor={editor}>
      <PlateContent />
    </Plate>
  );
}

describe('raw_markdown rendering', () => {
  it('renders the markdown source of a degraded block instead of empty space', () => {
    render(<Harness />);
    const block = screen.getByTestId('raw-markdown-block');
    expect(block).toHaveTextContent('See [[guide|the guide]] and ![icon](a.png) inline.');
  });

  it('serializes back to its verbatim source for the local mirror', () => {
    const rule = rawMarkdownMdRules[RAW_MARKDOWN_KEY];
    expect(
      rule.serialize({
        type: RAW_MARKDOWN_KEY,
        markdown: '| a | b |\n| - | - |',
        children: [{ text: '' }],
      })
    ).toEqual({ type: 'html', value: '| a | b |\n| - | - |' });
  });
});
