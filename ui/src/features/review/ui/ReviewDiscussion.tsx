import { useState } from 'react';
import { useNativeReview } from '../native-review-context';
import { ReviewAuthorHeader } from './ReviewAuthorHeader';
import { ActionsMenu, CommentEditForm } from './CommentThreadCard';

export function ReviewDiscussion({ parent, active }: { parent: string; active: boolean }) {
  const review = useNativeReview();
  const [draft, setDraft] = useState('');
  const [focused, setFocused] = useState(false);
  const [editing, setEditing] = useState<string>();
  const [menu, setMenu] = useState<string>();
  const replies = review.document.comments.filter((view) => !view.comment.deleted && view.comment.parent_id === parent);
  const submit = () => {
    if (!draft.trim() || review.readOnly) return;
    if (review.command({ op: 'reply_comment', id: crypto.randomUUID(), parent, author: review.author, body: draft.trim() })) setDraft('');
  };
  return <>
    {replies.length > 0 && <div className="mt-5 flex flex-col gap-5">{replies.map(({ comment }) => <div key={comment.id} className="group/reply">
      <div className="flex items-start justify-between gap-2">
        <ReviewAuthorHeader by={comment.author} at={comment.metadata.created_at} editedAt={comment.metadata.updated_at !== comment.metadata.created_at ? comment.metadata.updated_at : undefined} />
        <ActionsMenu label="Reply actions" canEdit open={menu === comment.id} onOpenChange={(open) => setMenu(open ? comment.id : undefined)}
          onEdit={() => setEditing(comment.id)} onDelete={() => review.command({ op: 'delete_comment', id: comment.id })} />
      </div>
      {editing === comment.id ? <CommentEditForm label="Edit reply" original={comment.body} onCancel={() => setEditing(undefined)}
        onSave={(body) => { if (review.command({ op: 'edit_comment', id: comment.id, body })) setEditing(undefined); }} />
        : <p className="mt-1 text-sm whitespace-pre-wrap text-body">{comment.body}</p>}
    </div>)}</div>}
    {active && <div className="mt-3 flex flex-col gap-2">
      <input aria-label="Reply" data-testid="reply-input" value={draft} disabled={review.readOnly}
        className="w-full rounded-md border border-line bg-raised px-2.5 py-1.5 text-sm text-ink outline-none focus:border-accent"
        onFocus={() => setFocused(true)} onBlur={() => setFocused(false)} onChange={(event) => setDraft(event.target.value)}
        onClick={(event) => event.stopPropagation()} onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); submit(); } }} placeholder="Reply…" />
      {(focused || draft.trim()) && <div className="flex items-center justify-end gap-2">
        <button aria-label="Cancel reply" className="rounded-md px-2 py-1.5 text-sm font-medium text-muted hover:text-body" onMouseDown={(event) => event.preventDefault()}
          onClick={(event) => { event.stopPropagation(); setDraft(''); review.setActiveId(null); }}>Cancel</button>
        <button aria-label="Submit reply" data-testid="reply-submit" disabled={review.readOnly || !draft.trim()}
          className="rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-on-accent hover:bg-accent-strong disabled:opacity-50"
          onClick={(event) => { event.stopPropagation(); submit(); }}>Reply</button>
      </div>}
    </div>}
  </>;
}
