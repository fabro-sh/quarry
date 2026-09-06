import { useEffect, useMemo, useRef } from 'react';
import { TextApi, type NodeEntry, type TRange } from 'platejs';
import { createPlatePlugin, PlateElement, PlateLeaf, type PlateElementProps, type PlateLeafProps, type PlateEditor } from 'platejs/react';
import type { Attachment, DocumentView, DocumentModel, ReviewTarget } from './document-model';
import { textOffset, type NativeOffset } from './plate-document-projection';
import { useReviewHighlights } from '../review/native-review-context';
import { cn } from '../../lib/utils';

function ReviewLeaf(props: PlateLeafProps) {
  const review = useReviewHighlights();
  const comments = props.leaf.quarryCommentIds as string[] ?? [];
  const suggestions = props.leaf.quarrySuggestionIds as string[] ?? [];
  const draft = props.leaf.quarryDraft === true;
  const formatting = props.leaf.quarryFormat === true;
  const insertion = props.leaf.quarryInsertion === true;
  const ids = [...comments, ...suggestions];
  const id = ids.includes(review.activeId ?? '') ? review.activeId : ids[0];
  return <PlateLeaf {...props} as="span" attributes={{ ...props.attributes, 'data-comment-id': comments[0], 'data-suggestion-id': suggestions[0], 'data-comment-draft': draft || undefined,
    onClick: () => { if (id) review.setActiveId(id); }, onMouseEnter: () => { if (id) review.setHoverId(id); }, onMouseLeave: () => review.setHoverId(null) } as never}
    className={cn('rounded-sm', comments.length > 0 && 'bg-warn-tint decoration-warn-line underline decoration-2 underline-offset-4',
      suggestions.length > 0 && (insertion ? 'bg-accent-tint text-accent-ink underline decoration-accent-line' : formatting ? 'bg-accent-tint underline decoration-accent-line' : 'text-danger line-through'),
      draft && 'bg-warn-tint ring-1 ring-warn-line',
      ids.some((id) => id === review.activeId || id === review.hoverId) && 'ring-1 ring-accent-ring')} />;
}

export const NativeReviewPlugin = createPlatePlugin({ key: 'quarry_review', node: { isLeaf: true }, render: { node: ReviewLeaf } });

function ProposalElement(props: PlateElementProps) {
  const review = useReviewHighlights();
  const id = String(props.element.quarryProposal);
  return <PlateElement {...props} as="span" attributes={{ ...props.attributes, 'data-suggestion-id': id,
    onClick: () => review.setActiveId(id), onMouseEnter: () => review.setHoverId(id), onMouseLeave: () => review.setHoverId(null) } as never}
    className={cn('rounded-sm bg-accent-tint text-accent-ink underline decoration-accent-line underline-offset-4',
      (review.activeId === id || review.hoverId === id) && 'ring-1 ring-accent-ring')} />;
}

export const NativeProposalPlugin = createPlatePlugin({ key: 'quarry_proposal', node: { isElement: true, isInline: true }, render: { node: ProposalElement } });

export function useReviewDecoration(model: DocumentModel, editor: PlateEditor, draftTarget?: ReviewTarget) {
  const draft = useRef(draftTarget); draft.current = draftTarget;
  const previousDraft = useRef(draftTarget);
  useEffect(() => {
    if (previousDraft.current === draftTarget) return;
    previousDraft.current = draftTarget;
    editor.api.redecorate();
    editor.api.onChange();
  }, [editor, draftTarget]);
  return useMemo(() => {
    let cachedDocument: DocumentView | undefined;
    let cachedDraft: ReviewTarget | undefined;
    let ranges: ReturnType<typeof collectRanges>;
    function collectRanges(document: DocumentView, draftTarget?: ReviewTarget) {
    const byBlock = new Map<string, Array<Attachment & { id: string; kind: 'comment' | 'suggestion' | 'format' | 'draft' }>>();
    const add = (part: Attachment, id: string, kind: 'comment' | 'suggestion' | 'format' | 'draft') => {
      const key = `${part.owner.kind}:${part.owner.id}`;
      const items = byBlock.get(key) ?? []; items.push({ ...part, id, kind }); byBlock.set(key, items);
    };
    for (const view of document.comments) {
      if (view.comment.deleted || view.comment.state !== 'open' || view.comment.parent_id) continue;
      for (const part of view.target.attachments) {
        add(part, view.comment.id, 'comment');
      }
    }
    for (const view of document.proposals) {
      if (view.proposal.state !== 'open') continue;
      for (const part of view.target.attachments) add(part, view.proposal.id, view.proposal.action.kind === 'format' ? 'format' : 'suggestion');
    }
    for (const part of draftTarget?.attachments ?? []) add(part, '', 'draft');
    return byBlock;
    }
    // Read the committed native state in Slate's render. Changing this callback
    // after the text render would split leaves after Slate positioned the DOM
    // selection, resetting Chrome's cursor as the text nodes change again.
    return ({ entry: [node, path] }: { entry: NodeEntry }): TRange[] => {
    const document = model.view();
    if (cachedDocument !== document || cachedDraft !== draft.current) {
      cachedDocument = document; cachedDraft = draft.current;
      ranges = collectRanges(document, draft.current);
    }
    if (!TextApi.isText(node) || !node.text) return [];
    let at: NativeOffset;
    try { at = textOffset(editor.children, { path, offset: 0 }); } catch { return []; }
    const parts = (ranges.get(`${at.proposal ? 'proposal' : 'block'}:${at.block}`) ?? []).flatMap((part) => {
      const start = Math.max(0, part.start - at.offset), end = Math.min(node.text.length, part.end - at.offset);
      return end > start ? [{ ...part, start, end }] : [];
    });
    if (at.proposedBlock) parts.push({ owner: { kind: 'proposal', id: at.block }, start: 0, end: node.text.length, quote: node.text, id: at.block, kind: 'suggestion' });
    const stops = [...new Set(parts.flatMap((part) => [part.start, part.end]))].sort((a, b) => a - b);
    return stops.slice(0, -1).flatMap((start, index) => {
      const end = stops[index + 1];
      const active = parts.filter((part) => part.start <= start && part.end >= end);
      return active.length ? [{ anchor: { path, offset: start }, focus: { path, offset: end }, quarry_review: true,
        quarryInsertion: !!at.proposedBlock,
        quarryFormat: active.some((part) => part.kind === 'format') && !active.some((part) => part.kind === 'suggestion'),
        quarryDraft: active.some((part) => part.kind === 'draft'),
        quarryCommentIds: active.filter((part) => part.kind === 'comment').map((part) => part.id),
        quarrySuggestionIds: active.filter((part) => part.kind === 'suggestion' || part.kind === 'format').map((part) => part.id) }] : [];
    });
    };
  }, [model, editor]);
}
