import type { Heading, Html, Nodes, Paragraph, PhrasingContent, TableCell } from 'mdast';
import type { MdxJsxTextElement } from 'mdast-util-mdx-jsx';
import type { Handle } from 'mdast-util-to-markdown';
import type { Plugin } from 'unified';
import type { Node } from 'unist';
import { visit } from 'unist-util-visit';

/*
 * Underline, subscript, and superscript have no plain-Markdown syntax, so they
 * round-trip as `<u>`, `<sub>`, `<sup>` HTML. We deliberately avoid the full
 * MDX/JSX micromark tokenizer (it escapes stray `<` and mangles `<url>`
 * autolinks); instead we:
 *   - serialize: a single to-markdown handler that prints the `mdxJsxTextElement`
 *     node `@platejs/markdown` emits for these marks as `<name>…</name>` (no
 *     global "unsafe" rules, so other prose is untouched);
 *   - parse: a targeted transformer that pairs the literal opening/closing inline
 *     HTML tags into an `mdxJsxTextElement`, which `@platejs/markdown` then maps
 *     back to the mark. Everything else is left exactly as is.
 */

const MARK_TAGS = new Set(['u', 'sub', 'sup']);

// Returns the mark tag name (e.g. "sub") if the node is one of our opening tags.
function openMarkTag(node: Nodes): string | null {
  if (node.type !== 'html') return null;
  const match = /^<([a-z]+)>$/i.exec(node.value.trim());
  const name = match?.[1].toLowerCase();
  return name && MARK_TAGS.has(name) ? name : null;
}

function isCloseTag(node: Nodes, name: string): node is Html {
  return node.type === 'html' && new RegExp(`^</${name}>$`, 'i').test(node.value.trim());
}

function rewritePhrasing(children: PhrasingContent[]): PhrasingContent[] {
  const result: PhrasingContent[] = [];
  for (let index = 0; index < children.length; index += 1) {
    const node = children[index];
    const name = openMarkTag(node);
    if (name) {
      const inner: PhrasingContent[] = [];
      let close = index + 1;
      while (close < children.length && !isCloseTag(children[close], name)) {
        inner.push(children[close]);
        close += 1;
      }
      if (close < children.length) {
        const element: MdxJsxTextElement = {
          type: 'mdxJsxTextElement',
          name,
          attributes: [],
          children: rewritePhrasing(inner),
        };
        result.push(element);
        index = close;
        continue;
      }
    }
    if ('children' in node) {
      node.children = rewritePhrasing(node.children);
    }
    result.push(node);
  }
  return result;
}

const serializeInlineMark: Handle = (node, _parent, state, info) => {
  if (node.type !== 'mdxJsxTextElement') return '';
  return `<${node.name}>${state.containerPhrasing(node, info)}</${node.name}>`;
};

function isPhrasingParent(node: Node): node is Heading | Paragraph | TableCell {
  return node.type === 'paragraph' || node.type === 'heading' || node.type === 'tableCell';
}

export const remarkInlineMarks: Plugin = function () {
  const data = this.data();
  (data.toMarkdownExtensions ??= []).push({ handlers: { mdxJsxTextElement: serializeInlineMark } });
  return (tree) => {
    visit(tree, isPhrasingParent, (node) => {
      node.children = rewritePhrasing(node.children);
    });
  };
};
