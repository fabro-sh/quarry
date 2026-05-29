import { markdownToPlateValue, plateValueToMarkdown } from './markdown-codec';

describe('markdown codec', () => {
  it('round-trips the supported markdown surface through Plate', () => {
    const markdown = [
      '# Heading',
      '',
      'A paragraph with **bold**, *italic*, `code`, and [a link](guide.md).',
      '',
      '> Quote',
      '',
      '- item one',
      '- item two',
      '',
      '---',
      '',
    ].join('\n');

    const value = markdownToPlateValue(markdown);

    expect(plateValueToMarkdown(value)).toContain('# Heading');
    expect(plateValueToMarkdown(value)).toContain('**bold**');
    expect(plateValueToMarkdown(value)).toContain('[a link](guide.md)');
  });
});
