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

  it('serializes raw_markdown blocks verbatim instead of dropping them', () => {
    const source = '- **CPU:** AMD Ryzen 5 7600 - $230\n- **PSU:** Corsair RM650 - $90';
    const value = [
      { type: 'p', children: [{ text: 'Shopping list:' }] },
      { type: 'raw_markdown', markdown: source, children: [{ text: '' }] },
      { type: 'p', children: [{ text: 'Total: $320.' }] },
    ];

    const markdown = plateValueToMarkdown(value);

    expect(markdown).toContain('Shopping list:');
    expect(markdown).toContain(source);
    expect(markdown).toContain('Total: $320.');
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

  it('round-trips image references', () => {
    const cases = [
      '![](assets/abc.png)\n',
      'Before.\n\n![](assets/y.jpg)\n\nAfter.\n',
      '![](data:image/png;base64,aGk=)\n',
    ];
    for (const md of cases) {
      expect(plateValueToMarkdown(markdownToPlateValue(md))).toBe(md);
    }
  });

  it('round-trips a mermaid block as a ```mermaid code fence', () => {
    const md = '```mermaid\ngraph TD\n  A --> B\n```\n';
    const value = markdownToPlateValue(md);
    expect((value[0] as { type?: string }).type).toBe('mermaid');
    expect((value[0] as { code?: string }).code).toBe('graph TD\n  A --> B');
    expect(plateValueToMarkdown(value)).toBe(md);
  });

  it('round-trips a GFM table with inline marks in cells', () => {
    const md = '| Name | Role |\n| --- | --- |\n| **Ana** | `dev` |\n';
    const value = markdownToPlateValue(md);
    expect((value[0] as { type?: string }).type).toBe('table');
    const out = plateValueToMarkdown(value);
    expect(out).toContain('| Name');
    expect(out).toContain('**Ana**');
    expect(out).toContain('`dev`');
    expect(plateValueToMarkdown(markdownToPlateValue(out))).toBe(out);
  });

  it('round-trips GFM column alignment (left/center/right)', () => {
    const md = '| L | C | R |\n| :-- | :-: | --: |\n| 1 | 2 | 3 |\n';
    const value = markdownToPlateValue(md);
    expect((value[0] as { align?: unknown }).align).toEqual(['left', 'center', 'right']);
    const out = plateValueToMarkdown(value);
    // remark-gfm emits the canonical minimal delimiter row, collapsing each
    // column to the narrowest valid form (`:-`/`:-:`/`-:`) — semantically the
    // same alignment markers as the `:--`/`--:` input.
    expect(out).toContain(':-');
    expect(out).toContain(':-:');
    expect(out).toContain('-:');
    // The alignment markers survive a full re-parse, which is what matters.
    expect((markdownToPlateValue(out)[0] as { align?: unknown }).align).toEqual([
      'left',
      'center',
      'right',
    ]);
  });

  it('drops table colSizes when serializing (resize is editor-only)', () => {
    const md = '| A | B |\n| --- | --- |\n| x | y |\n';
    const value = markdownToPlateValue(md);
    const withSizes = [{ ...(value[0] as Record<string, unknown>), colSizes: [200, 200] }, ...value.slice(1)];
    expect(plateValueToMarkdown(withSizes)).toBe(plateValueToMarkdown(value));
  });

  it('repairs malformed table values before serializing', () => {
    const value = [
      {
        type: 'table',
        align: ['left'],
        colSizes: [120, 120],
        children: [
          {
            type: 'tr',
            children: [
              { type: 'td', colSpan: 2, children: [{ type: 'p', children: [{ text: 'A' }] }] },
              { type: 'th', children: [{ type: 'p', children: [{ text: 'B' }] }] },
              { type: 'th', children: [{ type: 'p', children: [{ text: 'C' }] }] },
            ],
          },
          {
            type: 'tr',
            children: [
              { type: 'td', children: [{ type: 'p', children: [{ text: '1' }] }] },
              { type: 'td', children: [{ type: 'p', children: [{ text: '2' }] }] },
              { type: 'td', children: [{ type: 'p', children: [{ text: '3' }] }] },
              { type: 'td', rowSpan: 2, children: [{ type: 'p', children: [{ text: 'keep' }] }] },
            ],
          },
        ],
      },
    ];

    const out = plateValueToMarkdown(value as never);
    const reparsed = markdownToPlateValue(out);

    expect(out).toContain('keep');
    expect((reparsed[0] as { align?: unknown }).align).toEqual(['left', null, null, null]);
    expect(
      ((reparsed[0] as { children: Array<{ children: unknown[] }> }).children).map(
        (row) => row.children.length
      )
    ).toEqual([4, 4]);
  });

  it('drops upload placeholders when serializing', () => {
    const value = [
      { type: 'p', children: [{ text: 'Keep me.' }] },
      { type: 'placeholder', id: 'x', mediaType: 'img', children: [{ text: '' }] },
    ];
    expect(plateValueToMarkdown(value as never)).toBe('Keep me.\n');
  });

  it('keeps [[..]] literal inside code', () => {
    const md = '`[[notcode]]` stays.\n';
    expect(plateValueToMarkdown(markdownToPlateValue(md))).toBe(md);
  });

  it('leaves stray angle brackets and braces untouched', () => {
    const markdown = 'if a < b and {x} then y\n';
    expect(plateValueToMarkdown(markdownToPlateValue(markdown))).toBe(markdown);
  });

  // CommonMark break semantics: a soft break (word-wrap newline) is
  // collapsible whitespace, a hard break is a real line break. Mirrors the
  // Rust codec (crates/quarry-collab-codec) — parity is pinned by the
  // slate-yjs compat fixtures.
  it('joins soft-wrapped paragraph lines with spaces', () => {
    const value = markdownToPlateValue('para one\nwrapped line two\n');
    expect(value).toEqual([{ type: 'p', children: [{ text: 'para one wrapped line two' }] }]);
  });

  it('joins soft-wrapped lines inside emphasis keeping the mark', () => {
    const value = markdownToPlateValue('*foo\nbar*\n');
    expect(value).toEqual([{ type: 'p', children: [{ text: 'foo bar', italic: true }] }]);
  });

  it('serializes embedded newlines as backslash hard breaks', () => {
    const value = [{ type: 'p', children: [{ text: 'a\nb' }] }];
    expect(plateValueToMarkdown(value)).toBe('a\\\nb\n');
  });

  it('round-trips a backslash hard break', () => {
    const markdown = 'line one\\\nline two\n';
    expect(plateValueToMarkdown(markdownToPlateValue(markdown))).toBe(markdown);
  });

  it('normalizes a two-space hard break to a backslash break', () => {
    const value = markdownToPlateValue('line one  \nline two\n');
    expect(plateValueToMarkdown(value)).toBe('line one\\\nline two\n');
  });

  it('round-trips a hard break inside a list item', () => {
    // `*` bullet: Plate's serializer normalizes bullets to `*`, a pre-existing
    // divergence from the Rust writer's `-` (fixtures pin only the parse side).
    const markdown = '* first\\\n  second\n';
    expect(plateValueToMarkdown(markdownToPlateValue(markdown))).toBe(markdown);
  });

  it('round-trips a hard break inside a blockquote', () => {
    const markdown = '> first\\\n> second\n';
    expect(plateValueToMarkdown(markdownToPlateValue(markdown))).toBe(markdown);
  });
});
