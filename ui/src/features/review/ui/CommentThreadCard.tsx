import * as DropdownMenu from '@radix-ui/react-dropdown-menu';
import { Check, MoreHorizontal, Trash2 } from 'lucide-react';
import { nanoid } from 'nanoid';
import { useState } from 'react';

import { cn } from '../../../lib/utils';
import { currentAuthor } from '../identity';
import { formatRelativeTime, initials } from '../format';
import type { ReviewMeta, ReviewMetaEntry } from '../rfm-types';
import { addReply, deleteComment, resolveComment, useReviewStore, type ReviewThread } from '../review-store';

const menuItem =
  'flex w-full cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-left text-sm text-body outline-none hover:bg-well data-highlighted:bg-well';

function applyMeta(reducer: (meta: ReviewMeta) => ReviewMeta) {
  const store = useReviewStore.getState();
  store.setMeta(reducer(store.getMeta()));
}

function Avatar({ by }: { by: string }) {
  return (
    <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-well text-xs font-medium text-muted">
      {initials(by)}
    </span>
  );
}

function CommentHeader({ entry, badge }: { entry: ReviewMetaEntry; badge?: boolean }) {
  return (
    <div className="flex items-center gap-2">
      <Avatar by={entry.by} />
      <span className="text-sm font-medium text-ink">{entry.by}</span>
      <span className="text-xs text-faint">{formatRelativeTime(entry.at)}</span>
      {badge ? <span className="ml-1 text-xs font-medium text-muted">Resolved</span> : null}
    </div>
  );
}

export function CommentThreadCard({ thread }: { thread: ReviewThread }) {
  const activeId = useReviewStore((state) => state.activeId);
  const [draft, setDraft] = useState('');

  const resolved = thread.entry.status === 'resolved';
  const isActive = activeId === thread.id;

  function submitReply() {
    const body = draft.trim();
    if (!body) return;
    applyMeta((meta) =>
      addReply(meta, nanoid(), {
        parentId: thread.id,
        body,
        by: currentAuthor(),
        at: new Date().toISOString(),
      })
    );
    setDraft('');
  }

  function resolve() {
    applyMeta((meta) => resolveComment(meta, thread.id));
  }

  function remove() {
    applyMeta((meta) => deleteComment(meta, thread.id));
  }

  return (
    <div
      className={cn(
        'rounded-lg border bg-raised p-3',
        resolved ? 'border-line' : 'border-warn-line',
        isActive && 'ring-2 ring-accent-ring'
      )}
      onClick={() => useReviewStore.getState().setActiveId(thread.id)}
    >
      <div className="flex items-start justify-between gap-2">
        <CommentHeader entry={thread.entry} badge={resolved} />
        <DropdownMenu.Root>
          <DropdownMenu.Trigger asChild>
            <button
              aria-label="Comment actions"
              className="flex h-7 w-7 shrink-0 items-center justify-center rounded text-faint outline-none hover:bg-well hover:text-body"
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
              {resolved ? null : (
                <DropdownMenu.Item className={menuItem} data-testid="resolve-comment" onSelect={resolve}>
                  <Check className="shrink-0" size={15} />
                  Resolve
                </DropdownMenu.Item>
              )}
              <DropdownMenu.Item className={cn(menuItem, 'text-danger')} onSelect={remove}>
                <Trash2 className="shrink-0" size={15} />
                Delete
              </DropdownMenu.Item>
            </DropdownMenu.Content>
          </DropdownMenu.Portal>
        </DropdownMenu.Root>
      </div>

      <p className="mt-2 text-sm whitespace-pre-wrap text-body">{thread.entry.body ?? ''}</p>

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

      <div className="mt-3 flex flex-col gap-2">
        <textarea
          aria-label="Reply"
          className="min-h-9 w-full resize-y rounded-md border border-line bg-raised p-2 text-sm text-ink outline-none focus:border-accent"
          data-testid="reply-input"
          onChange={(event) => setDraft(event.target.value)}
          onClick={(event) => event.stopPropagation()}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && !event.shiftKey) {
              event.preventDefault();
              submitReply();
            }
          }}
          placeholder="Reply…"
          value={draft}
        />
        <button
          aria-label="Submit reply"
          className="self-end rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-on-accent transition-colors hover:bg-accent-strong disabled:cursor-not-allowed disabled:opacity-50"
          data-testid="reply-submit"
          disabled={draft.trim().length === 0}
          onClick={(event) => {
            event.stopPropagation();
            submitReply();
          }}
          type="button"
        >
          Reply
        </button>
      </div>
    </div>
  );
}
