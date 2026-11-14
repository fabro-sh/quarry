import type { Heading } from '@platejs/toc';
import { NodeApi, TextApi, type Descendant, type SlateEditor } from 'platejs';

const subtrees = new WeakMap<Descendant, Heading[]>();
const results = new WeakMap<SlateEditor, Heading[]>();

// Slate replaces edited nodes. Reuse the heading list for unchanged subtrees
// instead of walking every paragraph and text leaf on each keystroke.
function headingsIn(node: Descendant): Heading[] {
  const cached = subtrees.get(node);
  if (cached) return cached;
  const headings: Heading[] = [];
  if (!TextApi.isText(node)) {
    if (/^h[1-6]$/.test(node.type)) {
      const title = NodeApi.string(node);
      if (title) headings.push({ id: node.id as string, type: node.type, depth: Number(node.type[1]), title, path: [] });
    }
    node.children.forEach((child, index) => {
      for (const heading of headingsIn(child)) headings.push({ ...heading, path: [index, ...heading.path] });
    });
  }
  subtrees.set(node, headings);
  return headings;
}

export function queryHeadings(editor: SlateEditor): Heading[] {
  const headings: Heading[] = [];
  editor.children.forEach((child, index) => {
    for (const heading of headingsIn(child)) headings.push({ ...heading, path: [index, ...heading.path] });
  });
  const previous = results.get(editor);
  if (previous?.length === headings.length && headings.every((heading, index) => {
    const old = previous[index];
    return heading.id === old.id && heading.type === old.type && heading.title === old.title
      && heading.path.length === old.path.length && heading.path.every((part, index) => part === old.path[index]);
  })) return previous;
  results.set(editor, headings);
  return headings;
}
