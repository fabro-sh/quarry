import type { Heading, Paragraph, PhrasingContent, TableCell } from 'mdast';
import type { Handle } from 'mdast-util-to-markdown';
import type { Plugin } from 'unified';
import type { Node } from 'unist';
import { visit } from 'unist-util-visit';

/*
 * CommonMark break semantics, mirroring the Rust codec
 * (crates/quarry-collab-codec): a soft break (a word-wrap newline in the
 * source) is collapsible whitespace, a hard break is a literal `\n` inside
 * the text. Parity is pinned by the slate-yjs compat fixtures.
 *
 *   - parse: a transformer collapses soft-break newlines inside mdast text
 *     values to spaces and rewrites `break` nodes to `\n` text, merging
 *     adjacent text nodes. Normalizing breaks into text up front keeps them
 *     on the plain text path through `@platejs/markdown`, whose list-item
 *     deserialization otherwise mangles `break` nodes into raw source bytes.
 *   - serialize: a to-markdown `text` handler emits embedded `\n` as
 *     backslash hard breaks, so they survive re-parse instead of degrading
 *     to soft breaks (which now collapse to spaces).
 */

function normalizePhrasing(children: PhrasingContent[]): PhrasingContent[] {
  const result: PhrasingContent[] = [];
  for (const child of children) {
    let node: PhrasingContent = child;
    if (node.type === 'break') {
      node = { type: 'text', value: '\n' };
    } else if (node.type === 'text') {
      node = { ...node, value: node.value.replace(/[ \t]*\n[ \t]*/g, ' '), position: undefined };
    } else if ('children' in node) {
      node.children = normalizePhrasing(node.children);
    }
    const previous = result.at(-1);
    if (previous?.type === 'text' && node.type === 'text') {
      previous.value += node.value;
      previous.position = undefined;
    } else {
      result.push(node);
    }
  }
  return result;
}

const serializeTextWithHardBreaks: Handle = (node, _parent, state, info) => {
  if (node.type !== 'text') return '';
  const lines: string[] = node.value.split('\n');
  return lines
    .map((line, index) =>
      state.safe(line, {
        ...info,
        before: index === 0 ? info.before : '\n',
        after: index === lines.length - 1 ? info.after : '\n',
      })
    )
    .join('\\\n');
};

function isPhrasingParent(node: Node): node is Heading | Paragraph | TableCell {
  return node.type === 'paragraph' || node.type === 'heading' || node.type === 'tableCell';
}

export const remarkBreakSemantics: Plugin = function () {
  const data = this.data();
  (data.toMarkdownExtensions ??= []).push({
    handlers: { text: serializeTextWithHardBreaks },
  });
  return (tree) => {
    visit(tree, isPhrasingParent, (node) => {
      node.children = normalizePhrasing(node.children);
    });
  };
};
