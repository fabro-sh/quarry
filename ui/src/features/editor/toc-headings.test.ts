import { documentHeadings } from './toc-headings';
import type { Descendant } from 'platejs';

test('headings retain order, nested paths and inline text', () => {
  const nodes: Descendant[] = [
    { type: 'h1', id: 'title', children: [{ text: 'Title' }] },
    { type: 'blockquote', children: [
      { type: 'p', children: [{ text: 'Body' }] },
      { type: 'h3', id: 'nested', children: [{ text: 'Nested ' }, { type: 'a', children: [{ text: 'link' }] }] },
    ] },
    { type: 'h2', id: 'empty', children: [{ text: '' }] },
  ];
  expect(documentHeadings(nodes)).toEqual([
    { id: 'title', depth: 1, path: [0], title: 'Title', type: 'h1' },
    { id: 'nested', depth: 3, path: [1, 1], title: 'Nested link', type: 'h3' },
  ]);
  nodes.splice(0, 1);
  expect(documentHeadings(nodes)[0].path).toEqual([0, 1]);
});

test('paragraph text is not read while collecting headings', () => {
  const leaf = { get text(): string { throw new Error('Paragraph text must not be inspected'); } };
  expect(documentHeadings([{ type: 'p', children: [leaf] }])).toEqual([]);
});
