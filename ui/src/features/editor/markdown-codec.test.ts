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

  it('round-trips subscript and superscript as <sub>/<sup> HTML', () => {
    const value = markdownToPlateValue('H<sub>2</sub>O and x<sup>2</sup>\n');
    const serialized = plateValueToMarkdown(value);
    expect(serialized).toContain('<sub>2</sub>');
    expect(serialized).toContain('<sup>2</sup>');
  });

  it('round-trips wiki-links without escaping the brackets', () => {
    const cases = [
      'A [[foo]] link.\n',
      'See [[notes/bar|Bar]] now.\n',
      'Jump to [[guide#Setup]].\n',
      'An embed ![[image]] here.\n',
    ];
    for (const md of cases) {
      const out = plateValueToMarkdown(markdownToPlateValue(md));
      expect(out).toBe(md);
      expect(out).not.toContain('\\[');
    }
  });

  it('keeps [[..]] literal inside code', () => {
    const md = '`[[notcode]]` stays.\n';
    expect(plateValueToMarkdown(markdownToPlateValue(md))).toBe(md);
  });

  it('leaves stray angle brackets and braces untouched', () => {
    const markdown = 'if a < b and {x} then y\n';
    expect(plateValueToMarkdown(markdownToPlateValue(markdown))).toBe(markdown);
  });
});
