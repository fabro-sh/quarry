import type { Heading, Html, Nodes, Paragraph, PhrasingContent, TableCell } from 'mdast';
import type { MdxJsxTextElement } from 'mdast-util-mdx-jsx';
import type { Handle } from 'mdast-util-to-markdown';
import type { Plugin } from 'unified';
import type { Node } from 'unist';
import { visit } from 'unist-util-visit';

/*
 * Markdown has no underline syntax, so underline marks round-trip as `<u>…</u>`
 * HTML. We deliberately avoid the full MDX/JSX micromark tokenizer (it escapes
 * stray `<` and mangles `<url>` autolinks); instead we:
 *   - serialize: register a single to-markdown handler that prints the
 *     `mdxJsxTextElement` node `@platejs/markdown` emits for underline as
 *     `<name>…</name>` (no global "unsafe" rules, so other prose is untouched);
 *   - parse: a targeted transformer that pairs literal `<u>` / `</u>` inline
 *     HTML nodes into an `mdxJsxTextElement`, which `@platejs/markdown` then
 *     maps back to the underline mark. Everything else is left exactly as is.
 */

const OPEN = /^<u>$/i;
const CLOSE = /^<\/u>$/i;

function htmlMatches(node: Nodes, pattern: RegExp): node is Html {
  return node.type === 'html' && pattern.test(node.value.trim());
}

function rewritePhrasing(children: PhrasingContent[]): PhrasingContent[] {
  const result: PhrasingContent[] = [];
  for (let index = 0; index < children.length; index += 1) {
    const node = children[index];
    if (htmlMatches(node, OPEN)) {
      const inner: PhrasingContent[] = [];
      let close = index + 1;
      while (close < children.length && !htmlMatches(children[close], CLOSE)) {
        inner.push(children[close]);
        close += 1;
      }
      if (close < children.length) {
        const element: MdxJsxTextElement = {
          type: 'mdxJsxTextElement',
          name: 'u',
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

const serializeUnderline: Handle = (node, _parent, state, info) => {
  if (node.type !== 'mdxJsxTextElement') return '';
  return `<${node.name}>${state.containerPhrasing(node, info)}</${node.name}>`;
};

function isPhrasingParent(node: Node): node is Heading | Paragraph | TableCell {
  return node.type === 'paragraph' || node.type === 'heading' || node.type === 'tableCell';
}

export const remarkUnderline: Plugin = function () {
  const data = this.data();
  (data.toMarkdownExtensions ??= []).push({ handlers: { mdxJsxTextElement: serializeUnderline } });
  return (tree) => {
    visit(tree, isPhrasingParent, (node) => {
      node.children = rewritePhrasing(node.children);
    });
  };
};
