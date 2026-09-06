import { Check, X } from 'lucide-react';
import { useEditorSelector } from 'platejs/react';
import { useNativeReview } from '../review/native-review-context';
import { selectedText, textOffset } from './plate-document-projection';

export function NativeSuggestionActions() {
  const review = useNativeReview();
  const id = useEditorSelector((editor) => {
    if (!editor.selection) return null;
    let point;
    try { point = textOffset(editor.children, editor.selection.anchor); } catch { return null; }
    if (point.proposal) return point.block;
    const selected = selectedText(editor.children, editor.selection);
    const candidates = review.document.proposals.filter((view) => view.proposal.state === 'open' && view.target.attachments.some((part) =>
      selected.some((range) => range.block === part.owner.id && range.offset < part.end && range.end > part.start)
      || point.block === part.owner.id && point.offset >= part.start && point.offset <= part.end));
    return candidates.find((view) => view.proposal.id === review.activeId)?.proposal.id ?? candidates[0]?.proposal.id ?? null;
  }, [review.document, review.activeId]);
  const proposal = review.document.proposals.find((view) => view.proposal.id === id);
  if (!proposal || proposal.proposal.state !== 'open') return null;
  return <>
    <div aria-hidden="true" className="mx-0.5 h-5 w-px bg-line" />
    <button aria-label="Accept suggestion" title="Accept suggestion" disabled={review.readOnly || !!proposal.acceptance_error}
      className="inline-flex size-7 items-center justify-center rounded bg-accent-tint text-accent-ink hover:bg-accent-line disabled:opacity-50"
      onMouseDown={(event) => event.preventDefault()} onClick={() => review.command({ op: 'accept_proposal', id: proposal.proposal.id })}><Check size={15} /></button>
    <button aria-label="Reject suggestion" title="Reject suggestion" disabled={review.readOnly}
      className="inline-flex size-7 items-center justify-center rounded text-muted hover:bg-well hover:text-danger"
      onMouseDown={(event) => event.preventDefault()} onClick={() => review.command({ op: 'reject_proposal', id: proposal.proposal.id })}><X size={15} /></button>
  </>;
}
