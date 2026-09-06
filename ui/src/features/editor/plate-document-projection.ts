import { ElementApi, PointApi, RangeApi, TextApi, type Descendant, type NodeEntry, type Path, type Point, type TRange, type TElement, type TText } from 'platejs';
import type { BlockView, DocumentView, Owner, TextPoint, TextRun } from './document-model';
import capabilities from '../../../../crates/quarry-document/block-capabilities.json';
import { applyWikiLinks, sliceWikiLeaves, wikiLinkLeaves, wikiLinkMarkdown, type WikiLinkNode } from './wiki-link';

export type ReviewMarker = [kind: string, id: string, start: number, end: number];
export type PlateValue = TElement[];
export const blockKinds = new Map(capabilities.map((item) => [item.type, item]));
export type PlateBlock = TElement & { type: 'p' | 'h1' | 'h2' | 'h3' | 'h4' | 'h5' | 'h6' | 'blockquote' | 'code_block' | 'code_line' | 'table' | 'tr' | 'td' | 'th' | 'mermaid' | 'img' | 'hr' | 'raw_markdown' };
export const isBlock = (node: Descendant): node is PlateBlock => ElementApi.isElement(node) && blockKinds.has(node.type);
export const isTextBlock = (node: Descendant) => isBlock(node) && blockKinds.get(node.type)!.content === 'text';
// Plate normally replaces IDs on split, even when an operation supplies a
// fresh one. Native commands already assigned that identity; replacing it
// would copy the tail into a new block and hide its original comment targets.
export const nativeNodeIdOptions = {
  normalizeInitialValue: true, reuseId: true, disableInsertOverrides: true,
  idCreator: () => crypto.randomUUID(), filter: ([node]: NodeEntry) => blockKinds.has(String(node.type)),
};
export function inputBlockId(document: DocumentView) {
  let id = `input:${document.document_id}`;
  while (document.blocks.some((view) => view.block.id === id)) id += ':';
  return id;
}

export function inlineText(node: Descendant): string {
  if (TextApi.isText(node)) return node.text;
  if (node.type === 'quarry_proposal') return '';
  if (node.type === 'wikilink') return wikiLinkMarkdown(node as WikiLinkNode);
  return node.children.map(inlineText).join('');
}
export function blockText(node: TElement) { return node.children.map(inlineText).join(''); }
export function blockAttrs(node: TElement): Record<string, unknown> {
  const attrs: Record<string, unknown> = {};
  for (const [name, value] of Object.entries(node)) {
    if (name !== 'id' && name !== 'type' && name !== 'children' && !name.startsWith('quarry') && value !== undefined) attrs[name] = value;
  }
  return attrs;
}

export function runNodes(runs: TextRun[]): Descendant[] {
  const nodes: Descendant[] = [];
  let wiki: TText[] = [];
  const flushWiki = () => { if (wiki.length) nodes.push(...applyWikiLinks(wiki)); wiki = []; };
  for (const run of runs) {
    if (!run.text) continue;
    const { link, wikilink, ...marks } = run.marks;
    const text = { text: run.text, ...marks } as TText;
    if (wikilink === true) { wiki.push({ ...text, wikilink: true, ...(link === undefined ? {} : { link }) }); continue; }
    flushWiki();
    const last = nodes.at(-1);
    if (typeof link === 'string') {
      if (last && ElementApi.isElement(last) && last.type === 'a' && last.url === link) last.children.push(text);
      else nodes.push({ type: 'a', url: link, children: [text] });
    } else nodes.push(text);
  }
  flushWiki();
  return nodes.length ? nodes : [{ text: '' }];
}

/** Native blocks are the source of the Slate tree. IDs belong to blocks;
 * inline leaves and links are only a presentation of native text runs. */
export function projectPlate(document: DocumentView, input = true, locate?: (point: TextPoint) => { owner: Owner; offset: number } | null): PlateValue {
  const value = projectBlocks(document.blocks, input ? inputBlockId(document) : undefined);
  const blocks = new Map(flattenBlocks(value).map(({ node }) => [String(node.id), node]));
  const ids = new Set(blocks.keys());
  for (const view of document.proposals) {
    const action = view.proposal.action;
    if (view.proposal.state !== 'open' || action.kind !== 'insert_blocks') continue;
    const parent = action.parent ? blocks.get(String(action.parent)) : undefined;
    if (action.parent && !parent) continue;
    const siblings = parent ? parent.children : value;
    const proposal = projectBlocks(view.blocks);
    const original = new Map(view.blocks.map((block, index) => [block.block.id, { block, index }]));
    for (const { node } of flattenBlocks(proposal)) {
      const { block, index } = original.get(String(node.id))!;
      node.quarryProposal = view.proposal.id;
      node.quarryProposedBlock = node.id;
      node.quarryProposalOrder = index;
      node.quarrySources = block.block.segments.map((segment) => segment.source);
      // Proposed IDs are only reserved upon acceptance. Two proposals may use
      // the same ID, so give their presentation distinct Slate/DOM identities.
      let id = `proposal:${encodeURIComponent(view.proposal.id)}:${encodeURIComponent(String(node.id))}`;
      while (ids.has(id)) id += ':';
      ids.add(id); node.id = id;
    }
    const before = action.before ? siblings.findIndex((node) => node.id === action.before) : -1;
    const inputAt = siblings.findIndex((node) => node.id === inputBlockId(document));
    siblings.splice(before >= 0 ? before : inputAt >= 0 ? inputAt : siblings.length, 0, ...proposal);
  }
  const insertions = document.proposals.filter((view) => view.proposal.state === 'open' && view.proposal.action.kind === 'text')
    .map((view) => ({ view, at: locate?.(view.proposal.action.at as TextPoint) }))
    .filter((item) => item.at?.owner.kind === 'block')
    .sort((a, b) => b.at!.offset - a.at!.offset || b.view.proposal.id.localeCompare(a.view.proposal.id));
  for (const { view, at } of insertions) {
    const block = blocks.get(at!.owner.id);
    if (!block || !isTextBlock(block)) continue;
    block.children = insertInline(block.children, at!.offset, {
      type: 'quarry_proposal', quarryProposal: view.proposal.id, children: runNodes(view.runs),
    });
  }
  // External review changes must invalidate only their displayed owners. This
  // derived property makes Slate redraw those blocks without redecorating the
  // entire document. It is excluded from native attributes and copied content.
  const reviewKeys = new Map<string, ReviewMarker[]>();
  const add = (owner: Owner, value: ReviewMarker) => {
    const key = `${owner.kind}:${owner.id}`, entries = reviewKeys.get(key) ?? [];
    entries.push(value); reviewKeys.set(key, entries);
  };
  for (const { comment, target } of document.comments) {
    if (comment.deleted || comment.parent_id || comment.state !== 'open') continue;
    for (const part of target.attachments) add(part.owner, ['comment', comment.id, part.start, part.end]);
  }
  for (const { proposal, target } of document.proposals) {
    if (proposal.state !== 'open') continue;
    for (const part of target.attachments) add(part.owner, [proposal.action.kind, proposal.id, part.start, part.end]);
  }
  const annotateReview = (nodes: Descendant[], prefix: Path = []) => {
    for (const [index, node] of nodes.entries()) {
      if (!ElementApi.isElement(node)) continue;
      const path = [...prefix, index];
      if (node.type === 'wikilink') {
        const at = textOffset(value, { path: [...path, 0], offset: 0 });
        const entries = reviewKeys.get(`${at.proposal ? 'proposal' : 'block'}:${at.block}`) ?? [];
        node.quarryReviewKey = JSON.stringify(entries.filter(([, , start, end]) => start < at.offset + inlineText(node).length && end > at.offset));
        continue;
      }
      const key = node.quarryProposal ? `proposal:${node.quarryProposal}` : `block:${node.id}`;
      const entries = reviewKeys.get(key);
      if (entries) node.quarryReviewKey = JSON.stringify(entries);
      annotateReview(node.children, path);
    }
  };
  annotateReview(value);
  return value;
}

function insertInline(children: Descendant[], offset: number, node: TElement): Descendant[] {
  const result: Descendant[] = [];
  let inserted = false;
  for (const child of children) {
    const length = inlineText(child).length;
    if (inserted || offset > length || !length && !TextApi.isText(child)) {
      result.push(child); if (!inserted) offset -= length; continue;
    }
    if (TextApi.isText(child)) {
      result.push({ ...child, text: child.text.slice(0, offset) }, node, { ...child, text: child.text.slice(offset) });
    } else if (child.type === 'wikilink') {
      const leaves = wikiLinkLeaves(child as WikiLinkNode);
      result.push(...sliceWikiLeaves(leaves, 0, offset), node, ...sliceWikiLeaves(leaves, offset, inlineText(child).length));
    } else {
      result.push({ ...child, children: insertInline(child.children, offset, node) });
    }
    inserted = true;
  }
  if (!inserted) result.push(node, { text: '' });
  return result;
}
export function projectBlocks(views: BlockView[], emptyId?: string): PlateValue {
  const parents = new Map<string | null, BlockView[]>();
  for (const view of views) {
    const siblings = parents.get(view.block.parent) ?? []; siblings.push(view); parents.set(view.block.parent, siblings);
  }
  const children = (parent: string | null): PlateValue => (parents.get(parent) ?? [])
    .sort((a, b) => a.block.position - b.block.position || a.block.id.localeCompare(b.block.id))
    .map((view) => {
      const capability = blockKinds.get(view.block.kind);
      if (!capability) throw new Error(`Unsupported native block: ${view.block.kind}`);
      return { ...view.block.attrs, id: view.block.id, type: view.block.kind,
        children: capability.content === 'container' ? children(view.block.id)
          : capability.content === 'text' ? runNodes(view.runs) : [{ text: '' }] };
    });
  const result = children(null);
  if (emptyId && (!result.length || result.at(-1)!.type !== 'p')) result.push({ id: emptyId, type: 'p', children: [{ text: '' }] });
  return result;
}

export function nodeAt(value: Descendant[], path: Path): Descendant {
  let children = value; let node: Descendant | undefined;
  for (const index of path) {
    node = children[index];
    if (!node) throw new Error(`Missing editor node at ${path.join('.')}`);
    children = ElementApi.isElement(node) ? node.children : [];
  }
  if (!node) throw new Error('An editor node requires a path');
  return node;
}
export function blockAt(value: Descendant[], path: Path): { node: TElement; path: Path } | undefined {
  for (let length = path.length; length > 0; length--) {
    const prefix = path.slice(0, length); const node = nodeAt(value, prefix);
    if (isBlock(node)) return { node, path: prefix };
  }
}
export interface NativeOffset { block: string; offset: number; proposal?: boolean; proposedBlock?: string; blockOffset?: number }
export function proposedTextBlocks(value: Descendant[], proposal: string) {
  return flattenBlocks(value).filter(({ node }) => node.quarryProposal === proposal && isTextBlock(node))
    .sort((a, b) => Number(a.node.quarryProposalOrder) - Number(b.node.quarryProposalOrder));
}
export function textOffset(value: Descendant[], point: Point): NativeOffset {
  let block = blockAt(value, point.path);
  let proposal = false;
  for (let length = point.path.length - 1; length > 0; length--) {
    const path = point.path.slice(0, length), node = nodeAt(value, path);
    if (ElementApi.isElement(node) && node.type === 'quarry_proposal') {
      block = { node: { ...node, type: 'p', id: String(node.quarryProposal) }, path }; proposal = true; break;
    }
  }
  if (!block || !isTextBlock(block.node) || !block.node.id) throw new Error('Select a text block');
  let offset = point.offset; let children = block.node.children;
  for (const index of point.path.slice(block.path.length)) {
    offset += children.slice(0, index).reduce((sum, child) => sum + inlineText(child).length, 0);
    const node = children[index]; children = ElementApi.isElement(node) ? node.children : [];
  }
  if (block.node.quarryProposedBlock) {
    let start = 0;
    for (const { node } of proposedTextBlocks(value, String(block.node.quarryProposal))) {
      if (node.id === block.node.id) break;
      start += blockText(node).length;
    }
    return { block: String(block.node.quarryProposal), proposedBlock: String(block.node.quarryProposedBlock), blockOffset: offset, offset: start + offset, proposal: true };
  }
  return { block: String(block.node.id), offset, ...(proposal ? { proposal } : {}) };
}
export function slatePoint(value: Descendant[], blockId: string, offset: number, proposal = false, source?: string, proposedBlock?: { block: string; offset: number }): Point | undefined {
  if (proposal) {
    if (proposedBlock) {
      const block = proposedTextBlocks(value, blockId).find(({ node }) => node.quarryProposedBlock === proposedBlock.block);
      return block ? slatePoint(value, String(block.node.id), proposedBlock.offset) : undefined;
    }
    for (const { node } of proposedTextBlocks(value, blockId)) {
      const length = blockText(node).length;
      if (source ? (node.quarrySources as string[]).includes(source) : offset <= length) return slatePoint(value, String(node.id), offset);
      offset -= length;
    }
  }
  let found: Point | undefined;
  const visit = (nodes: Descendant[], prefix: Path, inside = false) => {
    for (let index = 0; index < nodes.length && !found; index++) {
      const node = nodes[index]; const path = [...prefix, index];
      if (!inside) { if (ElementApi.isElement(node)) visit(node.children, path, proposal ? node.quarryProposal === blockId : node.id === blockId); }
      else if (TextApi.isText(node)) {
        if (offset <= node.text.length) found = { path, offset }; else offset -= node.text.length;
      } else if (node.type === 'quarry_proposal') {
        continue;
      } else if (node.type === 'wikilink') {
        if (offset < inlineText(node).length) found = { path: [...path, 0], offset: 0 }; else offset -= inlineText(node).length;
      } else visit(node.children, path, true);
    }
  };
  visit(value, []); return found;
}

export interface FlatBlock { node: TElement; parent: string | null; position: number; path: Path }
export function flattenBlocks(value: Descendant[], canonical = false): FlatBlock[] {
  const result: FlatBlock[] = [];
  const visit = (nodes: Descendant[], parent: string | null, path: Path) => {
    let position = 0;
    nodes.forEach((node, index) => {
    if (!isBlock(node) || canonical && node.quarryProposal) return;
    const next = [...path, index]; result.push({ node, parent, position: canonical ? position++ : index, path: next });
    if (blockKinds.get(node.type)!.content === 'container') visit(node.children, String(node.id), next);
  });
  };
  visit(value, null, []); return result;
}

/** Collect the selected native owners in visible order, including proposal
 * text between canonical leaves. Never infer owners from repeated text. */
export function selectedText(value: Descendant[], selection: TRange): Array<NativeOffset & { end: number }> {
  const [start, end] = RangeApi.edges(selection);
  const result: Array<NativeOffset & { end: number }> = [];
  if (PointApi.equals(start, end)) return result;
  const visit = (nodes: Descendant[], prefix: Path) => nodes.forEach((node, index) => {
    const path = [...prefix, index];
    if (ElementApi.isElement(node) && node.type === 'wikilink') {
      const point = { path: [...path, 0], offset: 0 };
      if (PointApi.compare(point, start) >= 0 && PointApi.compare(point, end) < 0) {
        const at = textOffset(value, point);
        result.push({ ...at, end: at.offset + inlineText(node).length });
      }
      return;
    }
    if (!TextApi.isText(node)) { visit(node.children, path); return; }
    const from = { path, offset: 0 }, to = { path, offset: node.text.length };
    if (PointApi.compare(to, start) <= 0 || PointApi.compare(from, end) >= 0) return;
    const left = PointApi.compare(from, start) < 0 ? start.offset : 0;
    const right = PointApi.compare(to, end) > 0 ? end.offset : node.text.length;
    if (right <= left) return;
    let at: NativeOffset;
    try { at = textOffset(value, { path, offset: left }); } catch { return; }
    const previous = result.at(-1);
    if (previous?.block === at.block && previous.proposal === at.proposal && previous.proposedBlock === at.proposedBlock && previous.end === at.offset) previous.end += right - left;
    else result.push({ ...at, end: at.offset + right - left });
  });
  visit(value, []);
  return result;
}
