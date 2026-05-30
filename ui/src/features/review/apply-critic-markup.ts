import { nanoid } from 'nanoid';

import type { ReviewMeta } from './rfm-types';

type Node = Record<string, unknown>;

const CODE_BLOCK_TYPES = new Set(['code_block', 'code_line']);

// One regex with named groups for each marker family, plus an optional {#id}.
const TOKEN = new RegExp(
  [
    String.raw`\{==(?<hl>(?:(?!==\}).)*)==\}(?:\{>>(?<cbody>(?:(?!<<\}).)*)<<\})?(?:\{#(?<cid>[A-Za-z0-9_-]+)\})?`,
    String.raw`\{~~(?<sold>(?:(?!~>).)*)~>(?<snew>(?:(?!~~\}).)*)~~\}(?:\{#(?<subid>[A-Za-z0-9_-]+)\})?`,
    String.raw`\{\+\+(?<ins>(?:(?!\+\+\}).)*)\+\+\}(?:\{#(?<insid>[A-Za-z0-9_-]+)\})?`,
    String.raw`\{--(?<del>(?:(?!--\}).)*)--\}(?:\{#(?<delid>[A-Za-z0-9_-]+)\})?`,
    String.raw`\{>>(?<conly>(?:(?!<<\}).)*)<<\}(?:\{#(?<conlyid>[A-Za-z0-9_-]+)\})?`,
  ].join('|'),
  'g',
);

function ensureSuggestion(meta: ReviewMeta, id: string): { by: string; at: string } {
  const entry = meta.suggestions[id] ?? { by: 'unknown', at: new Date().toISOString() };
  meta.suggestions[id] = entry;
  return entry;
}

function ensureComment(meta: ReviewMeta, id: string, body?: string): void {
  const existing = meta.comments[id];
  const entry = existing ? { ...existing } : { by: 'unknown', at: new Date().toISOString() };
  if (body && !entry.body) entry.body = body;
  meta.comments[id] = entry;
}

/** Build a leaf from carried props (`rest`), mark props (`extra`), and text. */
function leaf(rest: Node, extra: Node, text: string): Node {
  return { ...rest, ...extra, text };
}

function suggestionExtra(id: string, type: 'insert' | 'remove', userId: string): Node {
  const extra: Node = { suggestion: true };
  extra[`suggestion_${id}`] = { id, type, userId, createdAt: 0 };
  return extra;
}

function commentExtra(id: string): Node {
  const extra: Node = { comment: true };
  extra[`comment_${id}`] = true;
  return extra;
}

/**
 * Split one text leaf into plain + marked leaves. Carries the leaf's other
 * props (e.g. bold) onto each produced segment. Emits no zero-length plain
 * segments. Returns the original leaf unchanged when no token matches.
 */
function expandLeaf(node: Node, text: string, meta: ReviewMeta): Node[] {
  const { text: _omit, ...rest } = node;
  const out: Node[] = [];
  let last = 0;

  const pushPlain = (slice: string) => {
    if (slice.length > 0) out.push({ ...rest, text: slice });
  };

  TOKEN.lastIndex = 0;
  for (let m = TOKEN.exec(text); m !== null; m = TOKEN.exec(text)) {
    pushPlain(text.slice(last, m.index));
    last = m.index + m[0].length;

    const g = m.groups ?? {};

    if (g.hl !== undefined) {
      if (g.cbody !== undefined || g.cid !== undefined) {
        const id = g.cid ?? nanoid();
        ensureComment(meta, id, g.cbody);
        out.push(leaf(rest, commentExtra(id), g.hl));
      } else {
        out.push(leaf(rest, { highlight: true }, g.hl));
      }
    } else if (g.sold !== undefined && g.snew !== undefined) {
      const id = g.subid ?? nanoid();
      const entry = ensureSuggestion(meta, id);
      out.push(leaf(rest, suggestionExtra(id, 'remove', entry.by), g.sold));
      out.push(leaf(rest, suggestionExtra(id, 'insert', entry.by), g.snew));
    } else if (g.ins !== undefined) {
      const id = g.insid ?? nanoid();
      const entry = ensureSuggestion(meta, id);
      out.push(leaf(rest, suggestionExtra(id, 'insert', entry.by), g.ins));
    } else if (g.del !== undefined) {
      const id = g.delid ?? nanoid();
      const entry = ensureSuggestion(meta, id);
      out.push(leaf(rest, suggestionExtra(id, 'remove', entry.by), g.del));
    } else if (g.conly !== undefined) {
      const id = g.conlyid ?? nanoid();
      ensureComment(meta, id, g.conly);
      out.push(leaf(rest, commentExtra(id), ' '));
    }
  }

  if (out.length === 0) return [node];
  pushPlain(text.slice(last));
  return out;
}

function isTextLeaf(node: Node): node is Node & { text: string } {
  return typeof node.text === 'string';
}

function childNodes(node: Node): Node[] | null {
  const children = node.children;
  if (!Array.isArray(children)) return null;
  return children.filter((child): child is Node => typeof child === 'object' && child !== null);
}

function walk(node: Node, inCode: boolean, meta: ReviewMeta): Node {
  const children = childNodes(node);
  if (children === null) return node;

  const nextInCode = inCode || CODE_BLOCK_TYPES.has(typeof node.type === 'string' ? node.type : '');
  const nextChildren: Node[] = [];
  for (const child of children) {
    if (isTextLeaf(child) && !nextInCode && child.code !== true) {
      nextChildren.push(...expandLeaf(child, child.text, meta));
    } else {
      nextChildren.push(walk(child, nextInCode, meta));
    }
  }
  return { ...node, children: nextChildren };
}

/**
 * Rewrite CriticMarkup tokens found within single text leaves into Plate review
 * marks. Returns a new value and a shallow copy of `meta` augmented with
 * synthesized ids and lifted inline comment bodies. Leaves inside code blocks or
 * marked `code` are left literal.
 */
export function applyCriticMarkup(value: Node[], meta: ReviewMeta): { value: Node[]; meta: ReviewMeta } {
  const nextMeta: ReviewMeta = { comments: { ...meta.comments }, suggestions: { ...meta.suggestions } };
  const root = walk({ type: 'root', children: value }, false, nextMeta);
  const children = childNodes(root);
  return { value: children ?? value, meta: nextMeta };
}
