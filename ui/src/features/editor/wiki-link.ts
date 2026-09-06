import {
  createSlatePlugin,
  type Descendant,
  type Path,
  type SlateEditor,
  type TElement,
  type TText,
  type Point,
} from 'platejs';
import { usesLiteralInlineSyntax } from './block-capabilities';

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
  quarrySource?: string;
  quarryMarks?: Record<string, unknown>;
  quarryLeaves?: TText[];
  children: [TText];
}

// `![[embed]]`, `[[target]]`, `[[target#anchor]]`, `[[target|alias]]`, and any
// combination. `target` stops at the first `#`, `|`, or `]`; subpaths (`a/b`)
// are part of the target. Mirrors the backend's split_alias / split_anchor.
const WIKILINK_RE = /(!?)\[\[([^\]|#\r\n]+)(?:#([^\]|\r\n]+))?(?:\|([^\]\r\n]+))?]]/g;

function isText(node: Descendant): node is TText {
  return typeof (node as { text?: unknown }).text === 'string';
}

function wikiLinkFromMatch(match: RegExpExecArray, marks: Record<string, unknown> = {}): WikiLinkNode {
  const [, bang, target, anchor, alias] = match;
  const node: WikiLinkNode = { type: WIKILINK_KEY, target: target.trim(), quarrySource: match[0], quarryMarks: marks, children: [{ text: '' }] };
  if (anchor) node.anchor = anchor.trim();
  if (alias) node.alias = alias.trim();
  if (bang) node.embed = true;
  return node;
}

// Split a text leaf into [text · wikilink · text · …], preserving the leaf's
// marks on the surrounding text. Inline code keeps `[[...]]` literal.
export function wikiLinkLeaves(node: WikiLinkNode): TText[] {
  return node.quarryLeaves ?? [{ ...node.quarryMarks, text: wikiLinkMarkdown(node) }];
}

export function sliceWikiLeaves(leaves: TText[], start: number, end: number): TText[] {
  let offset = 0;
  return leaves.flatMap((leaf) => {
    const from = Math.max(start - offset, 0), to = Math.min(end - offset, leaf.text.length);
    offset += leaf.text.length;
    return from < to ? [{ ...leaf, text: leaf.text.slice(from, to) }] : [];
  });
}

function splitText(leaves: TText[]): Descendant[] {
  const text = leaves.map((leaf) => leaf.text).join('');
  if (!text.includes('[[')) return leaves;
  const out: Descendant[] = [];
  let last = 0;
  WIKILINK_RE.lastIndex = 0;
  for (let match = WIKILINK_RE.exec(text); match; match = WIKILINK_RE.exec(text)) {
    if (match.index > last) out.push(...sliceWikiLeaves(leaves, last, match.index));
    const parts = sliceWikiLeaves(leaves, match.index, match.index + match[0].length);
    const marks = Object.fromEntries(Object.entries(parts[0]).filter(([key, value]) => key !== 'text' && parts.every((part) => part[key] === value)));
    out.push({ ...wikiLinkFromMatch(match, marks), quarryLeaves: parts });
    last = match.index + match[0].length;
  }
  if (out.length === 0) return leaves;
  if (last < text.length) out.push(...sliceWikiLeaves(leaves, last, text.length));
  return out;
}

function splitChildren(children: Descendant[]): Descendant[] {
  const out: Descendant[] = [];
  let leaves: TText[] = [];
  const flush = () => { out.push(...splitText(leaves)); leaves = []; };
  for (const child of children) {
    if (isText(child) && (child.code !== true || child.wikilink === true)) { leaves.push(child); continue; }
    flush();
    if (isText(child) || usesLiteralInlineSyntax((child as TElement).type) || (child as TElement).type === WIKILINK_KEY) {
      out.push(child);
    } else {
      out.push({ ...child, children: splitChildren((child as TElement).children) });
    }
  }
  flush();
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
  if (node.quarrySource) return node.quarrySource;
  const anchor = node.anchor ? `#${node.anchor}` : '';
  const alias = node.alias ? `|${node.alias}` : '';
  return `${node.embed ? '!' : ''}[[${node.target}${anchor}${alias}]]`;
}

/** The text shown inside the chip. */
export function wikiLinkDisplay(node: WikiLinkNode): string {
  if (node.alias) return node.alias;
  return node.anchor ? `${node.target}#${node.anchor}` : node.target;
}

/** Preserve formatting on the visible label without exposing the syntax. */
export function wikiLinkDisplayLeaves(node: WikiLinkNode): TText[] {
  const source = wikiLinkMarkdown(node), leaves = wikiLinkLeaves(node);
  const match = FIRST_WIKILINK_RE.exec(source);
  if (!match) return [{ text: wikiLinkDisplay(node) }];
  const trim = (start: number, text: string) => sliceWikiLeaves(leaves, start + text.length - text.trimStart().length, start + text.trimEnd().length);
  if (match[4]) return trim(source.length - 2 - match[4].length, match[4]);
  const start = match[1].length + 2;
  return [...trim(start, match[2]), ...(match[3] ? [
    ...sliceWikiLeaves(leaves, start + match[2].length, start + match[2].length + 1),
    ...trim(start + match[2].length + 1, match[3]),
  ] : [])];
}

const FIRST_WIKILINK_RE = /(!?)\[\[([^\]|#\r\n]+)(?:#([^\]|\r\n]+))?(?:\|([^\]\r\n]+))?]]/;

/**
 * Live conversion: if a text node holds a complete `[[...]]`, replace that span
 * with a wiki-link node and drop the cursor right after it. Returns true when it
 * converted (the caller should skip its own normalize so Slate re-runs and
 * catches any further matches). Used by the editor's normalizeNode override so a
 * link becomes a chip as soon as you close `]]`.
 */
export function convertWikiLinkInText(editor: SlateEditor, node: TText, path: Path, preserve?: (at: Point, text: string, action: () => void) => void): boolean {
  if (node.code === true || !node.text.includes('[[')) return false;
  if (editor.api.above({ at: path, match: (n) => usesLiteralInlineSyntax((n as TElement).type) })) return false;
  const match = FIRST_WIKILINK_RE.exec(node.text);
  if (!match) return false;
  const start = { offset: match.index, path };
  const end = { offset: match.index + match[0].length, path };
  const selection = editor.selection;
  const selected = !!selection && [selection.anchor, selection.focus].every((point) =>
    point.path.join('.') === path.join('.') && point.offset >= start.offset && point.offset <= end.offset);
  const convert = () => editor.tf.withoutNormalizing(() => {
    editor.tf.delete({ at: { anchor: start, focus: end } });
    // Insert the chip plus a trailing text node (à la Plate's inline-void date
    // node); insertNodes advances the cursor past it, so typing continues after.
    const { text: _text, ...marks } = node;
    editor.tf.insertNodes([wikiLinkFromMatch(match, marks), { text: '' }], { at: start, select: selected });
  });
  if (preserve) preserve(start, match[0], convert); else convert();
  return true;
}

export const BaseWikiLinkPlugin = createSlatePlugin({
  key: WIKILINK_KEY,
  node: { isElement: true, isInline: true, isVoid: true, isMarkableVoid: true },
});

// MarkdownPlugin serialize rule: emit the link as a raw `html` mdast node so the
// brackets survive instead of being escaped.
export const wikiLinkMdRules = {
  [WIKILINK_KEY]: {
    serialize: (node: WikiLinkNode) => ({ type: 'html' as const, value: wikiLinkMarkdown(node) }),
  },
};
