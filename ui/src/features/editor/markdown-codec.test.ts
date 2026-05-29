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

  it('round-trips GFM task lists', () => {
    const value = markdownToPlateValue('- [ ] Todo 1\n- [x] Todo 2\n');
    const serialized = plateValueToMarkdown(value);
    expect(serialized).toContain('[ ] Todo 1');
    expect(serialized).toContain('[x] Todo 2');
  });

  it('round-trips underline as <u> HTML', () => {
    const value = markdownToPlateValue('Plain <u>under</u> text.\n');
    expect(plateValueToMarkdown(value)).toContain('<u>under</u>');
  });

  it('leaves stray angle brackets and braces untouched', () => {
    const markdown = 'if a < b and {x} then y\n';
    expect(plateValueToMarkdown(markdownToPlateValue(markdown))).toBe(markdown);
  });
});
