import type { PlateEditor } from 'platejs/react';
import { useState } from 'react';

import { cancelCommentDraft, commitCommentDraft } from '../comment-draft';

// The draft composer lives at the top of the review rail while a comment draft
// is active. Submitting promotes the draft to a real comment with the typed
// body; Cancel discards the draft. Nothing is persisted until Submit, so a
// bare draft never reaches the saved Markdown.
export function DraftCommentComposer({ editor, anchorText }: { editor: PlateEditor; anchorText: string }) {
  const [body, setBody] = useState('');

  function submit() {
    const text = body.trim();
    if (!text) return;
    commitCommentDraft(editor, text);
    setBody('');
  }

  function cancel() {
    cancelCommentDraft(editor);
    setBody('');
  }

  return (
    <div className="rounded-lg border border-warn-line bg-raised p-3" data-testid="draft-composer">
      {anchorText ? (
        <p className="mb-2 truncate text-xs text-muted">
          Commenting on: <span className="text-body">&ldquo;{anchorText}&rdquo;</span>
        </p>
      ) : null}
      <div className="flex flex-col gap-2">
        <textarea
          aria-label="Comment"
          autoFocus
          className="min-h-9 w-full resize-y rounded-md border border-line bg-raised p-2 text-sm text-ink outline-none focus:border-accent"
          data-testid="draft-input"
          onChange={(event) => setBody(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && !event.shiftKey) {
              event.preventDefault();
              submit();
            }
          }}
          placeholder="Comment…"
          value={body}
        />
        <div className="flex justify-end gap-2">
          <button
            aria-label="Cancel comment"
            className="rounded-md px-3 py-1.5 text-sm font-medium text-muted transition-colors hover:bg-well"
            data-testid="draft-cancel"
            onClick={cancel}
            type="button"
          >
            Cancel
          </button>
          <button
            aria-label="Submit comment"
            className="rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-on-accent transition-colors hover:bg-accent-strong disabled:cursor-not-allowed disabled:opacity-50"
            data-testid="draft-submit"
            disabled={body.trim().length === 0}
            onClick={submit}
            type="button"
          >
            Comment
          </button>
        </div>
      </div>
    </div>
  );
}
