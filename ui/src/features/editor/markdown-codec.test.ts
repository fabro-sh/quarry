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

  it('round-trips GFM strikethrough', () => {
    const value = markdownToPlateValue('A ~~struck~~ word.');
    expect(plateValueToMarkdown(value)).toContain('~~struck~~');
  });

  it('round-trips headings from h1 through h6', () => {
    const markdown = '# One\n\n## Two\n\n### Three\n\n#### Four\n\n##### Five\n\n###### Six\n';
    expect(plateValueToMarkdown(markdownToPlateValue(markdown))).toContain('###### Six');
  });
});
