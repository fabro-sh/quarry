import { createSlatePlugin, KEYS, NodeApi, type Descendant, type TElement, type TText } from 'platejs';

// A Mermaid diagram is stored in markdown as a plain ```mermaid fenced code
// block (so it round-trips for free), but the editor models it as an atomic
// VOID block: the diagram is a single, non-editable unit, so backspace from an
// adjacent line deletes the whole thing and the cursor can't wander into the
// source. The source lives on the node as a `code` string, edited via a textarea.

export const MERMAID_KEY = 'mermaid';

export interface TMermaidElement extends TElement {
  type: typeof MERMAID_KEY;
  code: string;
  children: [TText];
}

/** Convert ```mermaid code blocks (from the codec) into atomic mermaid nodes. */
export function applyMermaid(value: Descendant[]): Descendant[] {
  return value.map((node) => {
    const element = node as TElement;
    if (element.type === KEYS.codeBlock && 'lang' in element && element.lang === 'mermaid') {
      const code = element.children.map((line) => NodeApi.string(line)).join('\n');
      return { type: MERMAID_KEY, code, children: [{ text: '' }] } satisfies TMermaidElement;
    }
    if (Array.isArray(element.children)) {
      return { ...node, children: applyMermaid(element.children) };
    }
    return node;
  });
}

export const BaseMermaidPlugin = createSlatePlugin({
  key: MERMAID_KEY,
  node: { isElement: true, isVoid: true },
}).overrideEditor(({ editor, tf: { deleteBackward, deleteForward } }) => ({
  transforms: {
    // Backspace at the start of the block after a diagram removes the diagram
    // (the void block is one unit), rather than nudging the cursor into it.
    deleteBackward(unit) {
      const block = editor.api.block();
      if (block && editor.api.isCollapsed() && editor.api.isStart(editor.selection?.anchor, block[1])) {
        const prev = editor.api.previous({ at: block[1] });
        if (prev && 'type' in prev[0] && prev[0].type === MERMAID_KEY) {
          editor.tf.removeNodes({ at: prev[1] });
          return;
        }
      }
      deleteBackward(unit);
    },
    // Delete at the end of the block before a diagram removes the diagram too.
    deleteForward(unit) {
      const block = editor.api.block();
      if (block && editor.api.isCollapsed() && editor.api.isEnd(editor.selection?.anchor, block[1])) {
        const next = editor.api.next({ at: block[1] });
        if (next && 'type' in next[0] && next[0].type === MERMAID_KEY) {
          editor.tf.removeNodes({ at: next[1] });
          return;
        }
      }
      deleteForward(unit);
    },
  },
}));

// Serialize a mermaid node back to a ```mermaid fenced code block.
export const mermaidMdRules = {
  [MERMAID_KEY]: {
    serialize: (node: TMermaidElement) => ({ lang: 'mermaid', type: 'code' as const, value: node.code }),
  },
};
