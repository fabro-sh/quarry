import { useNativeReview, type ReviewThread } from '../native-review-context';
import * as DropdownMenu from '@radix-ui/react-dropdown-menu';
import { Check, MoreHorizontal, Pencil, Trash2 } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';

import { cn } from '../../../lib/utils';
import { ReviewAuthorHeader } from './ReviewAuthorHeader';

const menuItem =
  'flex w-full cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-left text-sm text-body outline-none hover:bg-well data-highlighted:bg-well';

export function CommentEditForm({
  label,
  original,
  onCancel,
  onSave,
}: {
  label: string;
  original: string;
  onCancel: () => void;
  onSave: (body: string) => void;
}) {
  const [value, setValue] = useState(original);
  const body = value.trim();
  const review = useNativeReview();
  const disabled = review.readOnly || body.length === 0 || body === original.trim();

  return (
    <div className="mt-2 flex flex-col gap-2">
      <textarea
        aria-label={label}
        autoFocus
        className="min-h-20 w-full resize-y rounded-md border border-line bg-raised px-2.5 py-2 text-sm text-ink outline-none focus:border-accent"
        onChange={(event) => setValue(event.target.value)}
        onClick={(event) => event.stopPropagation()}
        value={value}
      />
      <div className="flex items-center justify-end gap-2">
        <button
          aria-label="Cancel edit"
          className="rounded-md px-2 py-1.5 text-sm font-medium text-muted transition-colors hover:text-body"
          onClick={(event) => {
            event.stopPropagation();
            onCancel();
          }}
          type="button"
        >
          Cancel
        </button>
        <button
          aria-label="Save edit"
          className="rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-on-accent transition-colors hover:bg-accent-strong disabled:cursor-not-allowed disabled:opacity-50"
          disabled={disabled}
          onClick={(event) => {
            event.stopPropagation();
            if (!disabled) onSave(body);
          }}
          type="button"
        >
          Save
        </button>
      </div>
    </div>
  );
}

export function ActionsMenu({
  label,
  open,
  onOpenChange,
  canEdit,
  onEdit,
  onDelete,
}: {
  label: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  canEdit: boolean;
  onEdit: () => void;
  onDelete: () => void;
}) {
  const { readOnly } = useNativeReview();
  return (
    <DropdownMenu.Root onOpenChange={onOpenChange} open={open}>
      <DropdownMenu.Trigger asChild>
        <button
          aria-label={label}
          disabled={readOnly}
          className="flex size-7 shrink-0 items-center justify-center rounded text-faint outline-none transition-colors hover:bg-well hover:text-body"
          onClick={(event) => event.stopPropagation()}
          type="button"
        >
          <MoreHorizontal size={16} />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="end"
          className="z-50 min-w-36 rounded-md border border-line bg-raised p-1 shadow-lg"
          onClick={(event) => event.stopPropagation()}
          sideOffset={6}
        >
          {canEdit ? (
            <DropdownMenu.Item className={menuItem} onSelect={onEdit}>
              <Pencil className="shrink-0" size={15} />
              Edit
            </DropdownMenu.Item>
          ) : null}
          <DropdownMenu.Item className={cn(menuItem, 'text-danger')} onSelect={onDelete}>
            <Trash2 className="shrink-0" size={15} />
            Delete
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

export function CommentThreadCard({ thread }: { thread: ReviewThread }) {
  const review = useNativeReview();
  const activeId = review.activeId;
  const hoverId = review.hoverId;
  const setHoverId = review.setHoverId;
  const [draft, setDraft] = useState('');
  const [editingId, setEditingId] = useState<string | null>(null);
  const [openMenuId, setOpenMenuId] = useState<string | null>(null);
  const [replyFocused, setReplyFocused] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  const resolved = thread.entry.status === 'resolved';
  const isActive = activeId === thread.id;
  const isHover = hoverId === thread.id;
  // A freshly created comment has no body yet: the first submit fills the root,
  // and only later submissions are replies.
  const rootHasBody = !!thread.entry.body;

  useEffect(() => {
    if (isActive) ref.current?.scrollIntoView({ block: 'nearest' });
  }, [isActive]);

  function submit() {
    const body = draft.trim();
    if (!body) return;
    const saved = rootHasBody
      ? review.command({ op: 'reply_comment', id: crypto.randomUUID(), parent: thread.id, body, author: review.author })
      : review.command({ op: 'edit_comment', id: thread.id, body });
    if (!saved) return;
    setDraft('');
  }

  function saveEdit(id: string, body: string) {
    if (review.command({ op: 'edit_comment', id, body })) setEditingId(null);
  }

  function resolve() {
    review.command({ op: 'resolve_comment', id: thread.id, resolved: !resolved });
  }

  function discard() {
    review.command({ op: 'delete_comment', id: thread.id });
  }

  function deleteReply(id: string) {
    review.command({ op: 'delete_comment', id });
  }

  return (
    <div
      className={cn(
        'group rounded-lg bg-well/40 p-3 transition-colors',
        isHover && !isActive && 'bg-well/70',
        isActive && 'bg-well/70 ring-2 ring-accent-ring'
      )}
      data-active={isActive ? 'true' : 'false'}
      data-hover={isHover ? 'true' : 'false'}
      data-testid="comment-card"
      aria-label={`Comment by ${thread.entry.by}`}
      onClick={() => review.setActiveId(thread.id)}
      onMouseEnter={() => setHoverId(thread.id)}
      onMouseLeave={() => setHoverId(null)}
      ref={ref}
    >
      <div className="flex items-start justify-between gap-2">
        <ReviewAuthorHeader
          by={thread.entry.by}
          at={thread.entry.at}
          editedAt={thread.entry.editedAt}
          resolved={resolved}
        />
        <div
          className={cn(
            'flex shrink-0 items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100',
            openMenuId === thread.id && 'opacity-100'
          )}
        >
          {!rootHasBody ? null : (
            <button
              disabled={review.readOnly}
              aria-label={resolved ? "Reopen comment" : "Resolve comment"}
              className="inline-flex size-7 items-center justify-center rounded bg-accent-tint text-accent-ink transition-colors outline-none hover:bg-accent-line hover:text-accent-ink"
              data-testid="resolve-comment"
              onClick={(event) => {
                event.stopPropagation();
                resolve();
              }}
              type="button"
            >
              {resolved ? <span className="text-xs">Reopen</span> : <Check size={16} />}
            </button>
          )}
          <ActionsMenu
            canEdit={!resolved && rootHasBody}
            label="Comment actions"
            onDelete={discard}
            onEdit={() => setEditingId(thread.id)}
            onOpenChange={(next) => setOpenMenuId(next ? thread.id : null)}
            open={openMenuId === thread.id}
          />
        </div>
      </div>

      {rootHasBody ? (
        editingId === thread.id ? (
          <CommentEditForm
            label="Edit comment"
            onCancel={() => setEditingId(null)}
            onSave={(body) => saveEdit(thread.id, body)}
            original={thread.entry.body ?? ''}
          />
        ) : (
          <p className="mt-2 text-sm whitespace-pre-wrap text-body">{thread.entry.body}</p>
        )
      ) : null}

      {thread.replies.length > 0 ? (
        <div className="mt-5 flex flex-col gap-5">
          {thread.replies.map((reply) => (
            <div className="group/reply" key={reply.id}>
              <div className="flex items-start justify-between gap-2">
                <ReviewAuthorHeader
                  by={reply.entry.by}
                  at={reply.entry.at}
                  editedAt={reply.entry.editedAt}
                />
                {!resolved && reply.entry.status !== 'resolved' ? (
                  <div
                    className={cn(
                      'shrink-0 opacity-0 transition-opacity group-hover/reply:opacity-100 focus-within:opacity-100',
                      openMenuId === reply.id && 'opacity-100'
                    )}
                  >
                    <ActionsMenu
                      canEdit
                      label="Reply actions"
                      onDelete={() => deleteReply(reply.id)}
                      onEdit={() => setEditingId(reply.id)}
                      onOpenChange={(next) => setOpenMenuId(next ? reply.id : null)}
                      open={openMenuId === reply.id}
                    />
                  </div>
                ) : null}
              </div>
              {editingId === reply.id ? (
                <CommentEditForm
                  label="Edit reply"
                  onCancel={() => setEditingId(null)}
                  onSave={(body) => saveEdit(reply.id, body)}
                  original={reply.entry.body ?? ''}
                />
              ) : (
                <p className="mt-1 text-sm whitespace-pre-wrap text-body">{reply.entry.body ?? ''}</p>
              )}
            </div>
          ))}
        </div>
      ) : null}

      {isActive || !rootHasBody ? (
        <div className="mt-3 flex flex-col gap-2">
          <input
            disabled={review.readOnly}
            aria-label={rootHasBody ? 'Reply' : 'Comment'}
            autoFocus={!rootHasBody}
            className="w-full rounded-md border border-line bg-raised px-2.5 py-1.5 text-sm text-ink outline-none focus:border-accent"
            data-testid="reply-input"
            onBlur={() => setReplyFocused(false)}
            onChange={(event) => setDraft(event.target.value)}
            onClick={(event) => event.stopPropagation()}
            onFocus={() => setReplyFocused(true)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                event.preventDefault();
                submit();
              }
            }}
            placeholder={rootHasBody ? 'Reply…' : 'Comment…'}
            type="text"
            value={draft}
          />
          {rootHasBody ? (
            replyFocused || draft.trim().length > 0 ? (
              <div className="flex items-center justify-end gap-2">
                <button
                  aria-label="Cancel reply"
                  className="rounded-md px-2 py-1.5 text-sm font-medium text-muted transition-colors hover:text-body"
                  data-testid="reply-cancel"
                  onClick={(event) => {
                    event.stopPropagation();
                    setDraft('');
                    review.setActiveId(null);
                  }}
                  onMouseDown={(event) => event.preventDefault()}
                  type="button"
                >
                  Cancel
                </button>
                <button
                  aria-label="Submit reply"
                  className="rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-on-accent transition-colors hover:bg-accent-strong disabled:cursor-not-allowed disabled:opacity-50"
                  data-testid="reply-submit"
                  disabled={review.readOnly || draft.trim().length === 0}
                  onClick={(event) => {
                    event.stopPropagation();
                    submit();
                  }}
                  type="button"
                >
                  Reply
                </button>
              </div>
            ) : null
          ) : (
            <div className="flex justify-end gap-2">
              <button
                aria-label="Discard comment"
                className="rounded-md px-3 py-1.5 text-sm font-medium text-muted transition-colors hover:bg-well"
                data-testid="reply-cancel"
                onClick={(event) => {
                  event.stopPropagation();
                  discard();
                }}
                type="button"
              >
                Cancel
              </button>
              <button
                aria-label="Submit comment"
                className="rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-on-accent transition-colors hover:bg-accent-strong disabled:cursor-not-allowed disabled:opacity-50"
                data-testid="reply-submit"
                disabled={review.readOnly || draft.trim().length === 0}
                onClick={(event) => {
                  event.stopPropagation();
                  submit();
                }}
                type="button"
              >
                Comment
              </button>
            </div>
          )}
        </div>
      ) : null}
    </div>
  );
}
