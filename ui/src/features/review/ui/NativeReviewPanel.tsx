import { ReviewDiscussion } from './ReviewDiscussion';
import { useState } from 'react';
import type { ReviewTarget } from '../../editor/document-model';
import { useNativeReview, type CommentDraft, type ReviewThread } from '../native-review-context';
import { CommentThreadCard } from './CommentThreadCard';
import { DraftCommentComposer } from './DraftCommentComposer';
import { SuggestionCard } from './SuggestionCard';

export function NativeReviewPanel({ draft, submitDraft, cancelDraft, acceptIncoming }: {
  draft?: CommentDraft;
  submitDraft(body: string): boolean;
  cancelDraft(): void;
  acceptIncoming(id: string): void;
}) {
  const review = useNativeReview();
  const [closed, setClosed] = useState(false);
  const comments = review.document.comments.filter((view) => !view.comment.deleted);
  const entry = ({ comment }: typeof comments[number]): ReviewThread['entry'] => ({
    body: comment.body, by: comment.author, at: comment.metadata.created_at,
    editedAt: comment.metadata.updated_at !== comment.metadata.created_at ? comment.metadata.updated_at : undefined,
    status: comment.state,
  });
  const roots = comments.filter((view) => !view.comment.parent_id && (closed || view.comment.state === 'open'));
  const proposals = review.document.proposals.filter((view) => closed || view.proposal.state === 'open');
  return <aside aria-label="Document review" className="flex h-full min-h-0 flex-col gap-3 overflow-auto px-4 py-4">
    {draft && <DraftCommentComposer anchorText={draft.quote} onSubmit={submitDraft} onCancel={cancelDraft} />}
    <label className="flex items-center gap-2 text-xs text-muted"><input type="checkbox" checked={closed} onChange={(event) => setClosed(event.target.checked)} />Show resolved comments and closed suggestions</label>
    {!draft && !roots.length && !proposals.length && <p className="py-6 text-center text-sm text-muted">Select text to add a comment.</p>}
    {roots.map((view) => <div key={view.comment.id}>
      <CommentThreadCard thread={{ id: view.comment.id, entry: entry(view), replies: comments.filter((reply) => reply.comment.parent_id === view.comment.id).map((reply) => ({ id: reply.comment.id, entry: entry(reply) })) }} />
      <TargetStatus target={view.target} quote={view.comment.original_quote} />
    </div>)}
    {proposals.map((view) => <div key={view.proposal.id}>
      <SuggestionCard view={view} onAccept={(id) => review.command({ op: 'accept_proposal', id })} onReject={(id) => review.command({ op: 'reject_proposal', id })} />
      <TargetStatus target={view.target} />
    </div>)}
    {review.document.conflicts.filter((view) => closed || !view.resolved).map((conflict) => <section key={conflict.id} aria-label="Conflicting changes" onClick={() => review.setActiveId(conflict.id)} className="rounded-lg bg-well/40 p-3 text-sm">
      <p className="font-medium">Conflicting changes</p><p>Current version</p><pre className="overflow-auto">{conflict.canonical}</pre>
      <p>Incoming version</p><pre className="overflow-auto">{conflict.incoming}</pre>
      {!conflict.resolved && <div className="flex gap-3">
        <button disabled={review.readOnly} onClick={() => review.command({ op: 'resolve_conflict', id: conflict.id })}>Keep current</button>
        <button disabled={review.readOnly} onClick={() => acceptIncoming(conflict.id)}>Use incoming</button>
      </div>}
      <ReviewDiscussion parent={conflict.id} active={review.activeId === conflict.id} />
    </section>)}
  </aside>;
}

function TargetStatus({ target, quote }: { target: ReviewTarget; quote?: string }) {
  const review = useNativeReview();
  return <div className="px-3 text-xs text-muted">
    {target.state !== 'attached' && <p>{({ hidden: 'The target block was removed.', deleted: 'The target text was deleted.', unattached: 'The original target is unavailable.' })[target.state]}{quote && <> “{quote}”</>}</p>}
    {target.attachments.length > 0 && <button className="py-1 hover:text-accent-ink" onClick={() => review.focusTarget(target)}>Show target</button>}
  </div>;
}
