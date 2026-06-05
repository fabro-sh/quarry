import * as DropdownMenu from '@radix-ui/react-dropdown-menu';
import { Check, MoreHorizontal, Trash2 } from 'lucide-react';
import { nanoid } from 'nanoid';
import type { PlateEditor } from 'platejs/react';
import { useEffect, useRef, useState } from 'react';

import { cn } from '../../../lib/utils';
import { currentAuthor } from '../identity';
import { formatRelativeTime, initials } from '../format';
import { removeCommentMark } from '../remove-comment';
import type { ReviewMeta, ReviewMetaEntry } from '../rfm-types';
import { addReply, deleteComment, editComment, resolveComment, useReviewStore, type ReviewThread } from '../review-store';

const menuItem =
  'flex w-full cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-left text-sm text-body outline-none hover:bg-well data-highlighted:bg-well';

function applyMeta(reducer: (meta: ReviewMeta) => ReviewMeta) {
  const store = useReviewStore.getState();
  store.setMeta(reducer(store.getMeta()));
}

function Avatar({ by }: { by: string }) {
  return (
    <span className="flex size-7 shrink-0 items-center justify-center rounded-full bg-surface text-xs font-medium text-muted ring-1 ring-inset ring-line">
      {initials(by)}
    </span>
  );
}

function CommentHeader({ entry, badge }: { entry: ReviewMetaEntry; badge?: boolean }) {
  return (
    <div className="flex items-center gap-2.5">
      <Avatar by={entry.by} />
      <div className="flex min-w-0 flex-col">
        <span className="truncate text-sm font-medium leading-tight text-ink">{entry.by}</span>
        <span className="text-[11px] leading-tight text-faint">{formatRelativeTime(entry.at)}</span>
      </div>
      {badge ? <span className="ml-1 text-[11px] font-medium text-muted">Resolved</span> : null}
    </div>
  );
}

export function CommentThreadCard({ thread, editor }: { thread: ReviewThread; editor: PlateEditor }) {
  const activeId = useReviewStore((state) => state.activeId);
  const hoverId = useReviewStore((state) => state.hoverId);
  const setHoverId = useReviewStore((state) => state.setHoverId);
  const [draft, setDraft] = useState('');
  const [menuOpen, setMenuOpen] = useState(false);
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
    if (rootHasBody) {
      applyMeta((meta) =>
        addReply(meta, nanoid(), {
          parentId: thread.id,
          body,
          by: currentAuthor(),
          at: new Date().toISOString(),
        })
      );
    } else {
      applyMeta((meta) => editComment(meta, thread.id, body));
    }
    setDraft('');
  }

  function resolve() {
    applyMeta((meta) => resolveComment(meta, thread.id));
  }

  function discard() {
    removeCommentMark(editor, thread.id);
    applyMeta((meta) => deleteComment(meta, thread.id));
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
      onClick={() => useReviewStore.getState().setActiveId(thread.id)}
      onMouseEnter={() => setHoverId(thread.id)}
      onMouseLeave={() => setHoverId(null)}
      ref={ref}
    >
      <div className="flex items-start justify-between gap-2">
        <CommentHeader entry={thread.entry} badge={resolved} />
        <div
          className={cn(
            'flex shrink-0 items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100',
            menuOpen && 'opacity-100'
          )}
        >
          {resolved || !rootHasBody ? null : (
            <button
              aria-label="Resolve comment"
              className="inline-flex size-7 items-center justify-center rounded bg-accent-tint text-accent-ink transition-colors outline-none hover:bg-accent-line hover:text-accent-ink"
              data-testid="resolve-comment"
              onClick={(event) => {
                event.stopPropagation();
                resolve();
              }}
              type="button"
            >
              <Check size={16} />
            </button>
          )}
          <DropdownMenu.Root onOpenChange={setMenuOpen} open={menuOpen}>
            <DropdownMenu.Trigger asChild>
              <button
                aria-label="Comment actions"
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
                <DropdownMenu.Item className={cn(menuItem, 'text-danger')} onSelect={discard}>
                  <Trash2 className="shrink-0" size={15} />
                  Delete
                </DropdownMenu.Item>
              </DropdownMenu.Content>
            </DropdownMenu.Portal>
          </DropdownMenu.Root>
        </div>
      </div>

      {rootHasBody ? <p className="mt-2 text-sm whitespace-pre-wrap text-body">{thread.entry.body}</p> : null}

      {thread.replies.length > 0 ? (
        <div className="mt-3 flex flex-col gap-3 border-l border-line pl-3">
          {thread.replies.map((reply) => (
            <div key={reply.id}>
              <CommentHeader entry={reply.entry} />
              <p className="mt-1 text-sm whitespace-pre-wrap text-body">{reply.entry.body ?? ''}</p>
            </div>
          ))}
        </div>
      ) : null}

      {isActive || !rootHasBody ? (
        <div className="mt-3 flex flex-col gap-2">
          <input
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
                    useReviewStore.getState().setActiveId(null);
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
                  disabled={draft.trim().length === 0}
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
                disabled={draft.trim().length === 0}
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
