import { ElementApi, TextApi, type Descendant, type TText } from 'platejs';

import { cloneMeta, type ReviewMeta } from './rfm-types';
import { readSuggestionMark, type SuggestionMark } from './suggestion-mark';

type Props = Record<string, unknown>;

const CODE_BLOCK_TYPES = new Set(['code_block', 'code_line']);
const LEGACY_BLOCK_DELETE_TYPES = new Set([
  'p',
  'h1',
  'h2',
  'h3',
  'h4',
  'h5',
  'h6',
  'blockquote',
]);

function createdAtFromEntry(at: string | undefined): number {
  if (!at) return 0;
  const ms = Date.parse(at);
  return Number.isNaN(ms) ? 0 : ms;
}

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
  const entry = meta.suggestions[id] ?? { by: 'unknown', at: '' };
  meta.suggestions[id] = entry;
  return entry;
}

function ensureComment(meta: ReviewMeta, id: string, body?: string): void {
  const existing = meta.comments[id];
  const entry = existing ? { ...existing } : { by: 'unknown', at: '' };
  if (body && !entry.body) entry.body = body;
  meta.comments[id] = entry;
}

/** Build a text leaf from carried props (`rest`), mark props (`extra`), and text. */
function leaf(rest: Props, extra: Props, text: string): TText {
  return { ...rest, ...extra, text };
}

function suggestionExtra(id: string, type: 'insert' | 'remove', entry: { by: string; at: string }): Props {
  const extra: Props = { suggestion: true };
  extra[`suggestion_${id}`] = { id, type, userId: entry.by, createdAt: createdAtFromEntry(entry.at) };
  return extra;
}

function commentExtra(id: string): Props {
  const extra: Props = { comment: true };
  extra[`comment_${id}`] = true;
  return extra;
}

/**
 * Split one text leaf into plain + marked leaves. Carries the leaf's other
 * props (e.g. bold) onto each produced segment. Emits no zero-length plain
 * segments. Returns the original leaf unchanged when no token matches.
 */
function expandLeaf(node: TText, meta: ReviewMeta): TText[] {
  const { text, ...rest } = node;
  const out: TText[] = [];
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
        const id = g.cid;
        if (!id) {
          out.push(leaf(rest, {}, m[0]));
          continue;
        }
        ensureComment(meta, id, g.cbody);
        out.push(leaf(rest, commentExtra(id), g.hl));
      } else {
        out.push(leaf(rest, {}, g.hl));
      }
    } else if (g.sold !== undefined && g.snew !== undefined) {
      const id = g.subid;
      if (!id) {
        out.push(leaf(rest, {}, m[0]));
        continue;
      }
      const entry = ensureSuggestion(meta, id);
      out.push(leaf(rest, suggestionExtra(id, 'remove', entry), g.sold));
      out.push(leaf(rest, suggestionExtra(id, 'insert', entry), g.snew));
    } else if (g.ins !== undefined) {
      const id = g.insid;
      if (!id) {
        out.push(leaf(rest, {}, m[0]));
        continue;
      }
      const entry = ensureSuggestion(meta, id);
      out.push(leaf(rest, suggestionExtra(id, 'insert', entry), g.ins));
    } else if (g.del !== undefined) {
      const id = g.delid;
      if (!id) {
        out.push(leaf(rest, {}, m[0]));
        continue;
      }
      const entry = ensureSuggestion(meta, id);
      out.push(leaf(rest, suggestionExtra(id, 'remove', entry), g.del));
    } else if (g.conly !== undefined) {
      const id = g.conlyid;
      if (!id) {
        out.push(leaf(rest, {}, m[0]));
        continue;
      }
      ensureComment(meta, id, g.conly);
      out.push(leaf(rest, commentExtra(id), ' '));
    }
  }

  if (out.length === 0) return [node];
  pushPlain(text.slice(last));
  return out;
}

function fullTextRemoval(children: Descendant[]): SuggestionMark | null {
  const marks: SuggestionMark[] = [];
  const visit = (nodes: Descendant[]) => {
    for (const node of nodes) {
      if (TextApi.isText(node)) {
        if (node.text.length === 0) continue;
        const mark = readSuggestionMark(node);
        if (!mark || mark.type !== 'remove') return false;
        marks.push(mark);
      } else {
        const type = typeof node.type === 'string' ? node.type : '';
        if (type === 'img' || type === 'wikilink' || !visit(node.children)) return false;
      }
    }
    return true;
  };
  if (!visit(children) || marks.length === 0) return null;
  const first = marks[0];
  return marks.every((mark) => mark.id === first.id) ? first : null;
}

function walkChildren(
  value: Descendant[],
  inCode: boolean,
  meta: ReviewMeta,
  topLevel: boolean
): Descendant[] {
  const out: Descendant[] = [];
  for (const child of value) {
    if (ElementApi.isElement(child)) {
      const nextInCode = inCode || CODE_BLOCK_TYPES.has(typeof child.type === 'string' ? child.type : '');
      const children = walkChildren(child.children, nextInCode, meta, false);
      const next: Record<string, unknown> = { ...child, children };
      const blockType = typeof child.type === 'string' ? child.type : '';
      const removal =
        topLevel && LEGACY_BLOCK_DELETE_TYPES.has(blockType)
          ? fullTextRemoval(children)
          : null;
      if (removal) {
        next.suggestion = removal;
        const entry = ensureSuggestion(meta, removal.id);
        meta.suggestions[removal.id] = { ...entry, kind: 'block_delete' };
      }
      out.push(next as Descendant);
    } else if (TextApi.isText(child) && !inCode && child.code !== true) {
      out.push(...expandLeaf(child, meta));
    } else {
      out.push(child);
    }
  }
  return out;
}

/**
 * Rewrite CriticMarkup tokens found within single text leaves into Plate review
 * marks. Returns a new value and a shallow copy of `meta` normalized with
 * deterministic fallback entries and lifted inline comment bodies. Leaves
 * inside code blocks or marked `code` are left literal.
 */
export function applyCriticMarkup(value: Descendant[], meta: ReviewMeta): { value: Descendant[]; meta: ReviewMeta } {
  const nextMeta = cloneMeta(meta);
  return { value: walkChildren(value, false, nextMeta, true), meta: nextMeta };
}
