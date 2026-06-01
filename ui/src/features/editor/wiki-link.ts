import {
  createSlatePlugin,
  type Descendant,
  type Path,
  type SlateEditor,
  type TElement,
  type TText,
} from 'platejs';

// Obsidian-style wiki-links. The Rust backend already parses `[[...]]` into
// outgoing links; this mirrors its syntax in the editor so links render, round-
// trip, and navigate. Modeled as a void inline element (an atomic chip) so the
// display can't desync from the target.

export const WIKILINK_KEY = 'wikilink';

export interface WikiLinkNode extends TElement {
  type: typeof WIKILINK_KEY;
  target: string;
  alias?: string;
  anchor?: string;
  embed?: boolean;
  children: [TText];
}

// `![[embed]]`, `[[target]]`, `[[target#anchor]]`, `[[target|alias]]`, and any
// combination. `target` stops at the first `#`, `|`, or `]`; subpaths (`a/b`)
// are part of the target. Mirrors the backend's split_alias / split_anchor.
const WIKILINK_RE = /(!?)\[\[([^\]|#]+)(?:#([^\]|]+))?(?:\|([^\]]+))?]]/g;

// Element types whose text is literal (no wiki-link parsing inside code).
const OPAQUE_TYPES = new Set(['code_block', 'code_line']);

function isText(node: Descendant): node is TText {
  return typeof (node as { text?: unknown }).text === 'string';
}

function wikiLinkFromMatch(match: RegExpExecArray): WikiLinkNode {
  const [, bang, target, anchor, alias] = match;
  const node: WikiLinkNode = { type: WIKILINK_KEY, target: target.trim(), children: [{ text: '' }] };
  if (anchor) node.anchor = anchor.trim();
  if (alias) node.alias = alias.trim();
  if (bang) node.embed = true;
  return node;
}

// Split a text leaf into [text · wikilink · text · …], preserving the leaf's
// marks on the surrounding text. Inline code keeps `[[...]]` literal.
function splitText(leaf: TText): Descendant[] {
  if (leaf.code === true || !leaf.text.includes('[[')) return [leaf];
  const out: Descendant[] = [];
  let last = 0;
  WIKILINK_RE.lastIndex = 0;
  for (let match = WIKILINK_RE.exec(leaf.text); match; match = WIKILINK_RE.exec(leaf.text)) {
    if (match.index > last) out.push({ ...leaf, text: leaf.text.slice(last, match.index) });
    out.push(wikiLinkFromMatch(match));
    last = match.index + match[0].length;
  }
  if (out.length === 0) return [leaf];
  if (last < leaf.text.length) out.push({ ...leaf, text: leaf.text.slice(last) });
  return out;
}

function splitChildren(children: Descendant[]): Descendant[] {
  const out: Descendant[] = [];
  for (const child of children) {
    if (isText(child)) {
      out.push(...splitText(child));
    } else if (OPAQUE_TYPES.has((child as TElement).type) || (child as TElement).type === WIKILINK_KEY) {
      out.push(child);
    } else {
      out.push({ ...child, children: splitChildren((child as TElement).children) });
    }
  }
  return out;
}

/**
 * Convert any `[[...]]` text into wiki-link nodes. Idempotent (existing
 * wiki-link nodes pass through). Run on load so links render, and on serialize
 * so a typed-but-not-yet-converted `[[...]]` still round-trips instead of being
 * escaped to `\[\[...]]`.
 */
export function applyWikiLinks(value: Descendant[]): Descendant[] {
  return splitChildren(value);
}

/** The exact `[[...]]` markdown for a wiki-link node. */
export function wikiLinkMarkdown(node: WikiLinkNode): string {
  const anchor = node.anchor ? `#${node.anchor}` : '';
  const alias = node.alias ? `|${node.alias}` : '';
  return `${node.embed ? '!' : ''}[[${node.target}${anchor}${alias}]]`;
}

/** The text shown inside the chip. */
export function wikiLinkDisplay(node: WikiLinkNode): string {
  if (node.alias) return node.alias;
  return node.anchor ? `${node.target}#${node.anchor}` : node.target;
}

const FIRST_WIKILINK_RE = /(!?)\[\[([^\]|#]+)(?:#([^\]|]+))?(?:\|([^\]]+))?]]/;

/**
 * Live conversion: if a text node holds a complete `[[...]]`, replace that span
 * with a wiki-link node and drop the cursor right after it. Returns true when it
 * converted (the caller should skip its own normalize so Slate re-runs and
 * catches any further matches). Used by the editor's normalizeNode override so a
 * link becomes a chip as soon as you close `]]`.
 */
export function convertWikiLinkInText(editor: SlateEditor, node: TText, path: Path): boolean {
  if (node.code === true || !node.text.includes('[[')) return false;
  if (editor.api.above({ at: path, match: (n) => OPAQUE_TYPES.has((n as TElement).type) })) return false;
  const match = FIRST_WIKILINK_RE.exec(node.text);
  if (!match) return false;
  const start = { offset: match.index, path };
  const end = { offset: match.index + match[0].length, path };
  editor.tf.withoutNormalizing(() => {
    editor.tf.delete({ at: { anchor: start, focus: end } });
    // Insert the chip plus a trailing text node (à la Plate's inline-void date
    // node); insertNodes advances the cursor past it, so typing continues after.
    editor.tf.insertNodes([wikiLinkFromMatch(match), { text: '' }]);
  });
  return true;
}

export const BaseWikiLinkPlugin = createSlatePlugin({
  key: WIKILINK_KEY,
  node: { isElement: true, isInline: true, isVoid: true },
});

// MarkdownPlugin serialize rule: emit the link as a raw `html` mdast node so the
// brackets survive instead of being escaped.
export const wikiLinkMdRules = {
  [WIKILINK_KEY]: {
    serialize: (node: WikiLinkNode) => ({ type: 'html' as const, value: wikiLinkMarkdown(node) }),
  },
};
