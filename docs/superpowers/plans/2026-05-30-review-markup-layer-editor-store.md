# Review Markup Layer — Plan 2: Editor + Store Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire Plate's comment, suggestion, and highlight plugins into quarry's live editor so review marks load from / save to Markdown through the Plan 1 RFM codec, with a client-side discussion store, live "Suggesting" mode, and accept/reject — but only minimal controls (the polished rail/cards are Plan 3).

**Architecture:** The editor value holds the marks (`comment_<id>`, `suggestion_<id>`, `highlight`); a React store holds the comment-thread metadata. Because attribution is free-form `by:` strings (no user registry) and comment bodies are Markdown text, **the store is just a `ReviewMeta` (from Plan 1) held in React state** — no separate `TDiscussion`/`TComment` model. Suggestion marks are self-describing (`userId`/`createdAt` inline), so only comments need the store. `PlateMarkdownEditor` loads via `markdownToReview` (value → editor, meta → store) and saves via `reviewToMarkdown(value, storeMeta)` on either editor or store change. The Plan 1 codec is the only thing that touches Markdown; the live editor never serializes review marks itself.

**Tech Stack:** TypeScript (ESM), Vitest + Testing Library (jsdom), Playwright (e2e). `platejs` 52, `@platejs/comment` + `@platejs/suggestion` (new, pinned 52.0.11), `@platejs/basic-nodes` (highlight; already present), `zustand` (already a dep). **`ui/` uses bun** — `bun add`, `bunx vitest`, `bun run typecheck`, `bun run test:e2e`. (See [[quarry-ui-uses-bun]].)

**Builds on Plan 1** (merged): `ui/src/features/review/` provides `markdownToReview(md): { value, meta }`, `reviewToMarkdown(value, meta): string`, `rfm-types.ts` (`ReviewMeta`, `ReviewMetaEntry`, `emptyReviewMeta`), and the codec internals. **Reference design:** `docs/superpowers/specs/2026-05-30-review-markup-layer-design.md`.

**Verified Plate 52 API (transcribe exactly):**
- `import { CommentPlugin } from '@platejs/comment/react'`; `import { getCommentKey, getDraftCommentKey } from '@platejs/comment'`.
- `import { SuggestionPlugin } from '@platejs/suggestion/react'`; `import { acceptSuggestion, rejectSuggestion, getSuggestionKey, keyId2SuggestionId, type TResolvedSuggestion } from '@platejs/suggestion'`.
- `import { HighlightPlugin } from '@platejs/basic-nodes/react'`; toggle via `editor.tf.highlight.toggle()`.
- `import { KEYS } from 'platejs'` → `KEYS.comment`/`KEYS.suggestion`/`KEYS.highlight`.
- Suggesting mode: `editor.getOption(SuggestionPlugin, 'isSuggesting')` / `editor.setOption(SuggestionPlugin, 'isSuggesting', true)`. **GOTCHA:** set `currentUserId` (non-null) BEFORE enabling `isSuggesting`, or `withSuggestion`'s normalizer strips the new marks.
- Draft → comment promotion: `editor.getApi(CommentPlugin).comment.nodes({ at: [], isDraft: true })`, then per `[, path]`: `editor.tf.setNodes({ [getCommentKey(id)]: true }, { at: path, split: true })` and `editor.tf.unsetNodes([getDraftCommentKey()], { at: path })`.
- accept/reject need a `TResolvedSuggestion` (minimum `keyId` + `suggestionId`); wrap in `editor.getApi(SuggestionPlugin).suggestion.withoutSuggestions(() => acceptSuggestion(editor, desc))`.

---

## File structure (Plan 2)

| File | Responsibility |
|---|---|
| `ui/src/features/review/identity.ts` | `currentAuthor(): string` — free-form `by:` label (default `"user"`). |
| `ui/src/features/review/review-store.ts` | Zustand store holding `ReviewMeta`; actions (addComment/addReply/editComment/resolveComment/deleteComment); `hydrate`/`getMeta`; pure helpers `syncSuggestionsFromValue`, `buildThreads`. |
| `ui/src/features/review/resolve-suggestions.ts` | `resolveSuggestionsFromValue(editor)` → `TResolvedSuggestion[]` (transcribed from Plate's `toResolvedSuggestion`); used for accept/reject. |
| `ui/src/features/editor/review-kit.ts` | The comment + suggestion + highlight plugin array (configured), to spread into the editor. |
| `ui/src/features/review/apply-critic-markup.ts` | (modify) derive suggestion-leaf `createdAt` from `entry.at`. |
| `ui/src/features/editor/PlateMarkdownEditor.tsx` | (modify) route load/save through the codec + store; add review-kit; set `currentUserId`; save on value-or-store change. |
| Co-located `*.test.ts(x)` + one `e2e/*.spec.ts`. |

**Milestones:** Tasks 1–6 deliver **comments + highlights** round-tripping through the editor. Tasks 7–10 add **suggestions + track-changes**. The plan can be paused after Task 6 with working, shippable software.

---

### Task 1: Identity + dependencies

**Files:** Create `ui/src/features/review/identity.ts`, `ui/src/features/review/identity.test.ts`; Modify `ui/package.json`.

- [ ] **Step 1: Add deps** (pin to match the existing `@platejs/*` 52.0.11 line; verify the imports resolve in that version):

```bash
cd ui && bun add @platejs/comment@52.0.11 @platejs/suggestion@52.0.11
```

Expected: both appear under `dependencies`; `ui/bun.lock` updates. If 52.0.11 is unavailable, use the version matching the installed `@platejs/basic-nodes` (`cat ui/package.json | grep basic-nodes`) and report the chosen version.

- [ ] **Step 2: Write the failing test** `ui/src/features/review/identity.test.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { currentAuthor } from './identity';

describe('currentAuthor', () => {
  it('defaults to "user"', () => {
    expect(currentAuthor()).toBe('user');
  });
});
```

- [ ] **Step 3: Run test → fails.** `cd ui && bunx vitest run src/features/review/identity.test.ts` → `Failed to resolve import './identity'`.

- [ ] **Step 4: Implement** `ui/src/features/review/identity.ts`:

```ts
// The free-form author label stamped on review items created in this editor.
// Quarry has no user accounts; humans are "user" and agents write their own
// `by:` label directly into the Markdown. Centralized so Plan 3 / future config
// can override it.
const DEFAULT_AUTHOR = 'user';

export function currentAuthor(): string {
  return DEFAULT_AUTHOR;
}
```

- [ ] **Step 5: Run test → passes** (1 test). **Step 6: Commit.**

```bash
git add ui/package.json ui/bun.lock ui/src/features/review/identity.ts ui/src/features/review/identity.test.ts
git commit -m "feat(review): add review plugins deps + author identity

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Derive suggestion `createdAt` from `at` (codec)

**Files:** Modify `ui/src/features/review/apply-critic-markup.ts`; Modify `ui/src/features/review/apply-critic-markup.test.ts`.

Plan 1 hardcodes parsed suggestion leaves to `createdAt: 0`. The suggestion UI (Plan 3) shows real times, and the store derives `at` from the mark. Derive `createdAt` from the endmatter `at` (epoch ms), falling back to `0` when `at` is missing/unparseable (keeps determinism — a missing timestamp is the only non-deterministic input and it stays `0`).

- [ ] **Step 1: Add the failing test** to `apply-critic-markup.test.ts`:

```ts
it('derives suggestion createdAt (epoch ms) from the endmatter at timestamp', () => {
  const meta = { comments: {}, suggestions: { s1: { by: 'AI', at: '2026-01-01T00:00:00.000Z' } } };
  const { value } = applyCriticMarkup([p([{ text: 'add {++more++}{#s1}' }])], meta);
  const leaf = (value[0] as { children: Record<string, unknown>[] }).children[1];
  expect((leaf.suggestion_s1 as { createdAt: number }).createdAt).toBe(Date.parse('2026-01-01T00:00:00.000Z'));
});

it('falls back to createdAt 0 when at is missing or unparseable', () => {
  const { value } = applyCriticMarkup([p([{ text: 'add {++x++}{#s9}' }])], { comments: {}, suggestions: { s9: { by: 'AI', at: 'not-a-date' } } });
  const leaf = (value[0] as { children: Record<string, unknown>[] }).children[1];
  expect((leaf.suggestion_s9 as { createdAt: number }).createdAt).toBe(0);
});
```

- [ ] **Step 2: Run → fails** (createdAt is currently `0`): `cd ui && bunx vitest run src/features/review/apply-critic-markup.test.ts -t createdAt`.

- [ ] **Step 3: Implement.** In `apply-critic-markup.ts`, where suggestion leaf data is built (the `suggestion_<id>` extra object with `createdAt: 0`), compute it from the entry's `at`. Add a helper near the top:

```ts
function createdAtFromEntry(at: string | undefined): number {
  if (!at) return 0;
  const ms = Date.parse(at);
  return Number.isNaN(ms) ? 0 : ms;
}
```

Then, in each place that creates a `suggestion_<id>` data object (insert, remove, and both halves of a substitution), replace `createdAt: 0` with `createdAt: createdAtFromEntry(meta-entry.at)`. The entry is the one returned by `ensureSuggestion(meta, id)` — capture it: e.g. `const entry = ensureSuggestion(nextMeta, id); ... createdAt: createdAtFromEntry(entry.at)` and `userId: entry.by`. (Both insert and remove halves of a substitution use the same entry → same `createdAt`.)

- [ ] **Step 4: Run → passes;** also run the full review suite to confirm no regression (round-trip/idempotence still green, since `at` round-trips and `createdAt` is not serialized): `cd ui && bunx vitest run src/features/review && bun run typecheck`.

- [ ] **Step 5: Commit.**

```bash
git add ui/src/features/review/apply-critic-markup.ts ui/src/features/review/apply-critic-markup.test.ts
git commit -m "feat(review): derive suggestion createdAt from endmatter timestamp

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: The review store (pure reducers + selectors)

**Files:** Create `ui/src/features/review/review-store.ts`, `ui/src/features/review/review-store.test.ts`.

The store holds a `ReviewMeta` and exposes pure helpers (tested here) plus a Zustand store (thin wrapper). Test the PURE functions directly (no React).

- [ ] **Step 1: Write the failing test** `ui/src/features/review/review-store.test.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { addComment, addReply, resolveComment, deleteComment, buildThreads, syncSuggestionsFromValue } from './review-store';
import { emptyReviewMeta } from './rfm-types';

const at = '2026-01-01T00:00:00.000Z';

describe('review-store reducers', () => {
  it('addComment inserts a root comment entry', () => {
    const meta = addComment(emptyReviewMeta(), 'c1', { by: 'user', at });
    expect(meta.comments.c1).toEqual({ by: 'user', at });
  });

  it('addReply inserts a reply with re + body', () => {
    let meta = addComment(emptyReviewMeta(), 'c1', { by: 'user', at });
    meta = addReply(meta, 'c2', { parentId: 'c1', body: 'sure', by: 'AI', at });
    expect(meta.comments.c2).toEqual({ by: 'AI', at, body: 'sure', re: 'c1' });
  });

  it('resolveComment sets status', () => {
    let meta = addComment(emptyReviewMeta(), 'c1', { by: 'user', at });
    meta = resolveComment(meta, 'c1', 'done');
    expect(meta.comments.c1.status).toBe('resolved');
    expect(meta.comments.c1.resolved).toBe('done');
  });

  it('deleteComment removes a comment and its replies', () => {
    let meta = addComment(emptyReviewMeta(), 'c1', { by: 'user', at });
    meta = addReply(meta, 'c2', { parentId: 'c1', body: 'x', by: 'AI', at });
    meta = deleteComment(meta, 'c1');
    expect(meta.comments).toEqual({});
  });

  it('does not mutate the input meta', () => {
    const original = emptyReviewMeta();
    addComment(original, 'c1', { by: 'user', at });
    expect(original.comments).toEqual({});
  });

  it('buildThreads groups replies under their root, sorted', () => {
    let meta = addComment(emptyReviewMeta(), 'c1', { by: 'user', at });
    meta = addReply(meta, 'c2', { parentId: 'c1', body: 'r1', by: 'AI', at });
    const threads = buildThreads(meta);
    expect(threads).toHaveLength(1);
    expect(threads[0].id).toBe('c1');
    expect(threads[0].replies.map((r) => r.id)).toEqual(['c2']);
  });

  it('syncSuggestionsFromValue adds entries for marks missing from meta', () => {
    const value = [{ type: 'p', children: [{ text: 'x', suggestion: true, suggestion_s1: { id: 's1', type: 'insert', userId: 'user', createdAt: Date.parse(at) } }] }];
    const meta = syncSuggestionsFromValue(emptyReviewMeta(), value);
    expect(meta.suggestions.s1).toEqual({ by: 'user', at });
  });

  it('syncSuggestionsFromValue does not override an existing entry', () => {
    const value = [{ type: 'p', children: [{ text: 'x', suggestion: true, suggestion_s1: { id: 's1', type: 'insert', userId: 'someone-else', createdAt: 0 } }] }];
    const meta = syncSuggestionsFromValue({ comments: {}, suggestions: { s1: { by: 'AI', at } } }, value);
    expect(meta.suggestions.s1).toEqual({ by: 'AI', at });
  });
});
```

- [ ] **Step 2: Run → fails** (`Failed to resolve import './review-store'`).

- [ ] **Step 3: Implement** `ui/src/features/review/review-store.ts`:

```ts
import { create } from 'zustand';
import type { Descendant } from 'platejs';
import { emptyReviewMeta, type ReviewMeta, type ReviewMetaEntry } from './rfm-types';

// ---- Pure reducers (no React) ----

function cloneMeta(meta: ReviewMeta): ReviewMeta {
  return { comments: { ...meta.comments }, suggestions: { ...meta.suggestions } };
}

export function addComment(meta: ReviewMeta, id: string, fields: { by: string; at: string; body?: string }): ReviewMeta {
  const next = cloneMeta(meta);
  const entry: ReviewMetaEntry = { by: fields.by, at: fields.at };
  if (fields.body) entry.body = fields.body;
  next.comments[id] = entry;
  return next;
}

export function addReply(meta: ReviewMeta, id: string, fields: { parentId: string; body: string; by: string; at: string }): ReviewMeta {
  const next = cloneMeta(meta);
  next.comments[id] = { by: fields.by, at: fields.at, body: fields.body, re: fields.parentId };
  return next;
}

export function editComment(meta: ReviewMeta, id: string, body: string): ReviewMeta {
  const existing = meta.comments[id];
  if (!existing) return meta;
  const next = cloneMeta(meta);
  next.comments[id] = { ...existing, body };
  return next;
}

export function resolveComment(meta: ReviewMeta, id: string, summary?: string): ReviewMeta {
  const existing = meta.comments[id];
  if (!existing) return meta;
  const next = cloneMeta(meta);
  const entry: ReviewMetaEntry = { ...existing, status: 'resolved' };
  if (summary) entry.resolved = summary;
  next.comments[id] = entry;
  return next;
}

export function deleteComment(meta: ReviewMeta, id: string): ReviewMeta {
  const next = cloneMeta(meta);
  delete next.comments[id];
  // Drop direct replies (single-level threading: replies always point at a root).
  for (const [key, entry] of Object.entries(next.comments)) {
    if (entry.re === id) delete next.comments[key];
  }
  return next;
}

export interface ReviewThread {
  id: string;
  entry: ReviewMetaEntry;
  replies: Array<{ id: string; entry: ReviewMetaEntry }>;
}

export function buildThreads(meta: ReviewMeta): ReviewThread[] {
  const roots: ReviewThread[] = [];
  for (const [id, entry] of Object.entries(meta.comments)) {
    if (!entry.re) roots.push({ id, entry, replies: [] });
  }
  for (const [id, entry] of Object.entries(meta.comments)) {
    if (entry.re) {
      const root = roots.find((r) => r.id === entry.re);
      if (root) root.replies.push({ id, entry });
    }
  }
  for (const root of roots) root.replies.sort((a, b) => a.entry.at.localeCompare(b.entry.at));
  roots.sort((a, b) => a.entry.at.localeCompare(b.entry.at));
  return roots;
}

// Add a suggestions entry for every suggestion mark present in `value` that is
// missing from `meta` (a live-created suggestion). Existing entries win (loaded
// suggestions keep their original by/at). Derives by/at from the mark itself.
export function syncSuggestionsFromValue(meta: ReviewMeta, value: Descendant[]): ReviewMeta {
  const next = cloneMeta(meta);
  const visit = (node: Record<string, unknown>) => {
    for (const key of Object.keys(node)) {
      if (!key.startsWith('suggestion_')) continue;
      const raw = node[key];
      if (typeof raw !== 'object' || raw === null) continue;
      const data: Record<string, unknown> = { ...raw };
      const id = data.id;
      const userId = data.userId;
      const createdAt = data.createdAt;
      if (typeof id === 'string' && !next.suggestions[id]) {
        const at = typeof createdAt === 'number' && createdAt > 0 ? new Date(createdAt).toISOString() : new Date().toISOString();
        next.suggestions[id] = { by: typeof userId === 'string' ? userId : 'user', at };
      }
    }
    const children = node.children;
    if (Array.isArray(children)) {
      for (const child of children) {
        if (typeof child === 'object' && child !== null) visit(child as Record<string, unknown>);
      }
    }
  };
  for (const node of value as Record<string, unknown>[]) visit(node);
  return next;
}

// ---- Zustand store (thin holder of the meta) ----

interface ReviewStoreState {
  meta: ReviewMeta;
  hydrate: (meta: ReviewMeta) => void;
  setMeta: (meta: ReviewMeta) => void;
  getMeta: () => ReviewMeta;
}

export const useReviewStore = create<ReviewStoreState>((set, get) => ({
  meta: emptyReviewMeta(),
  hydrate: (meta) => set({ meta }),
  setMeta: (meta) => set({ meta }),
  getMeta: () => get().meta,
}));
```

Note: `new Date().toISOString()` in `syncSuggestionsFromValue` is the one wall-clock input — it only fires for a brand-new suggestion lacking a numeric `createdAt`, and the resulting `at` is then persisted, so it's stable across reloads.

- [ ] **Step 4: Run → passes** (8 tests). **Step 5: typecheck** `cd ui && bun run typecheck`. **Step 6: Commit.**

```bash
git add ui/src/features/review/review-store.ts ui/src/features/review/review-store.test.ts
git commit -m "feat(review): review store (meta reducers, threads, suggestion sync)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: The review plugin kit

**Files:** Create `ui/src/features/editor/review-kit.ts`, `ui/src/features/editor/review-kit.test.ts`.

Assemble the three plugins. Render components (leaf styling, rail) are Plan 3 — here we only register the plugins so marks exist, suggesting works, and the `mod+shift+m` comment shortcut is bound. Keep the array order stable (marks are order-insensitive).

- [ ] **Step 1: Write the failing test** `ui/src/features/editor/review-kit.test.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { reviewKit } from './review-kit';

describe('reviewKit', () => {
  it('registers comment, suggestion, and highlight plugins', () => {
    const keys = reviewKit.map((p) => p.key);
    expect(keys).toContain('comment');
    expect(keys).toContain('suggestion');
    expect(keys).toContain('highlight');
  });
});
```

- [ ] **Step 2: Run → fails.** `cd ui && bunx vitest run src/features/editor/review-kit.test.ts`.

- [ ] **Step 3: Implement** `ui/src/features/editor/review-kit.ts`:

```ts
import { CommentPlugin } from '@platejs/comment/react';
import { SuggestionPlugin } from '@platejs/suggestion/react';
import { HighlightPlugin } from '@platejs/basic-nodes/react';

// Review-layer marks for the live editor. Rendering/UI (leaf styling, rail,
// toolbar) is Plan 3; this only registers the marks + the comment shortcut +
// enables suggesting mode (toggled elsewhere). currentUserId is set on the
// editor at mount (see PlateMarkdownEditor) BEFORE suggesting is enabled.
export const reviewKit = [
  CommentPlugin.configure({
    shortcuts: { setDraft: { keys: 'mod+shift+m' } },
  }),
  SuggestionPlugin,
  HighlightPlugin,
];
```

- [ ] **Step 4: Run → passes.** If `.key` isn't directly readable on a configured plugin in this Plate version, assert via `reviewKit.some((p) => p.key === 'comment')` or inspect the actual shape and adjust the test to read the real key field — keep the assertion meaningful (it must fail if a plugin is missing). **Step 5: typecheck. Step 6: Commit.**

```bash
git add ui/src/features/editor/review-kit.ts ui/src/features/editor/review-kit.test.ts
git commit -m "feat(review): review plugin kit (comment, suggestion, highlight)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Load + save through the codec (editor integration)

**Files:** Modify `ui/src/features/editor/PlateMarkdownEditor.tsx`; Modify `ui/src/features/editor/MarkdownEditor.test.tsx` (or add `PlateMarkdownEditor.review.test.tsx`).

This is the core integration. Today `PlateMarkdownEditor` initializes from `markdownToPlateValue(content)` and serializes via `plateValueToMarkdown(value)` in `onValueChange` (with `lastContentRef`/`lastSerializedRef` echo guards + `resetPlateEditor` on external change). Switch both ends to the review codec and the store.

- [ ] **Step 1: Write a failing integration test.** Render `PlateMarkdownEditor` with `content` containing a comment, assert the comment anchor renders as a marked span and that editing triggers an `onChange` whose Markdown still contains the CriticMarkup. Add to a new `ui/src/features/editor/PlateMarkdownEditor.review.test.tsx`:

```tsx
import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { PlateMarkdownEditor } from './PlateMarkdownEditor';

const DOC = 'See {==here==}{>>fix this<<}{#c1}.\n\n---\ncomments:\n  c1:\n    at: "2026-01-01T00:00:00.000Z"\n    by: user\n';

describe('PlateMarkdownEditor review round-trip', () => {
  it('renders a commented range as a comment mark', () => {
    render(<PlateMarkdownEditor content={DOC} onChange={vi.fn()} />);
    // The commented text "here" is present in the editor.
    expect(screen.getByText('here')).toBeInTheDocument();
    // It carries the comment mark class/attribute Plate applies (data-slate-* on a comment leaf).
    const marked = document.querySelector('[class*="comment"], [data-comment-ids], .slate-comment');
    expect(marked).not.toBeNull();
  });
});
```

(The exact selector for a comment leaf depends on Plate's default `CommentPlugin` rendering — when you run this, inspect the rendered DOM and assert on the real attribute/class Plate emits for a `comment` leaf. Keep the assertion meaningful: it must fail if the comment mark isn't applied. If Plate's bare `CommentPlugin` renders no distinguishing attribute without a `node` render component, register a minimal `node` renderer in `review-kit.ts` that adds `data-comment` and assert on that — note this in your report.)

- [ ] **Step 2: Run → fails** (the editor doesn't parse review marks yet): `cd ui && bunx vitest run src/features/editor/PlateMarkdownEditor.review.test.tsx`.

- [ ] **Step 3: Implement the integration.** In `PlateMarkdownEditor.tsx`:

1. Imports:
```ts
import { markdownToReview, reviewToMarkdown } from '../review/rfm-codec';
import { reviewKit } from './review-kit';
import { useReviewStore, syncSuggestionsFromValue } from '../review/review-store';
import { currentAuthor } from '../review/identity';
import { SuggestionPlugin } from '@platejs/suggestion/react';
import type { PlateValue } from './markdown-codec';
```
2. Add `...reviewKit` to the `plateMarkdownPlugins` array (after the existing marks, before `MarkdownPlugin`). Keep `MarkdownPlugin` as-is — the live editor does NOT serialize review marks; the codec does.
3. Replace the initial value + serialization with the review codec. Where the component currently computes `initialValueRef` from `markdownToPlateValue(content)`, instead:
```ts
const storeHydrate = useReviewStore((s) => s.hydrate);
const storeGetMeta = useReviewStore((s) => s.getMeta);

const initialValueRef = useRef<PlateValue | null>(null);
if (!initialValueRef.current) {
  const { value, meta } = markdownToReview(content);
  initialValueRef.current = value as PlateValue;
  storeHydrate(meta);
}
```
4. Define the serializer used by `onValueChange` and the external-change effect:
```ts
// Pure: derives live-suggestion endmatter entries fresh from the value each
// call (idempotent — never persisted), so onValueChange and the store
// subscription can share it without re-entrancy. The store only persistently
// holds COMMENT metadata; suggestion entries are re-derivable from the marks.
const serialize = useCallback(
  (value: PlateValue): string =>
    reviewToMarkdown(value as never, syncSuggestionsFromValue(storeGetMeta(), value as never)),
  [storeGetMeta],
);
```
   Replace `plateValueToMarkdown(...)` calls (in `lastSerializedRef` init, `onValueChange`, and the external-change effect) with `serialize(...)`. In the external-change `useEffect`, after `markdownToReview(content)`, both `resetPlateEditor(editor, nextValue)` AND `storeHydrate(meta)`:
```ts
useEffect(() => {
  if (content === lastContentRef.current) return;
  const { value, meta } = markdownToReview(content);
  resetPlateEditor(editor, value as PlateValue);
  storeHydrate(meta);
  lastContentRef.current = content;
  lastSerializedRef.current = reviewToMarkdown(value as never, meta);
}, [content, editor, storeHydrate]);
```
5. Set `currentUserId` before any suggesting (mount effect), so `withSuggestion` doesn't normalize marks away:
```ts
useEffect(() => {
  editor.setOption(SuggestionPlugin, 'currentUserId', currentAuthor());
}, [editor]);
```
6. **Save on store change too** (replies/resolves from Plan 3, and the synced-suggestion writes): subscribe to the store and re-run the same debounced save the editor uses. Add after the editor is created:
```ts
useEffect(() => {
  return useReviewStore.subscribe(() => {
    const md = serialize(editor.children as PlateValue); // same pure serializer as onValueChange
    if (md === lastSerializedRef.current) return;
    lastContentRef.current = md;
    lastSerializedRef.current = md;
    onChange(md);
  });
}, [editor, onChange, serialize]);
```
   (Keep the `editor.meta.resetting` guard logic intact in `onValueChange`. Both save paths now go through the same pure `serialize`, and the `lastSerializedRef` equality check absorbs any duplicate fire.)

- [ ] **Step 4: Run the test → passes.** Iterate on the comment-leaf selector against the real rendered DOM. Then run the broader editor suite to ensure no regression: `cd ui && bunx vitest run src/features/editor && bun run typecheck`.

- [ ] **Step 5: Manual verification (boundary behavior the unit test can't fully prove).** Run the app and confirm a real load/edit/save cycle:
```bash
cd ui && bun run dev
```
Open a doc containing the `DOC` markup above (via the app's file flow). Confirm: the commented text shows a mark; typing elsewhere saves Markdown that still contains `{==here==}{>>fix this<<}{#c1}` + the endmatter. Report what you observed.

- [ ] **Step 6: Commit.**

```bash
git add ui/src/features/editor/PlateMarkdownEditor.tsx ui/src/features/editor/PlateMarkdownEditor.review.test.tsx
git commit -m "feat(review): load/save review marks through the RFM codec + store

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Create a comment / toggle a highlight (minimal actions)

**Files:** Modify `ui/src/features/editor/PlateMarkdownEditor.tsx` (extend the existing floating toolbar); Modify/extend a test.

Add two selection actions to the existing `FloatingFormatToolbar`: **Highlight** (toggle) and **Comment** (create). Comment creation: set a draft on the selection, mint a nanoid id, promote the draft to that id, and add a store entry so the comment has `by`/`at` (and an empty body the user fills in — Plan 3 supplies the composer; here the body defaults to empty and the comment still round-trips with just `by`/`at`).

- [ ] **Step 1: Write the failing test** — extend `PlateMarkdownEditor.review.test.tsx`: render with plain content, programmatically select a word, click the toolbar "Comment" button, assert `onChange` fires with Markdown containing a `{==...==}{#c<id>}`-shaped anchor and a `comments:` endmatter block. (Selection in jsdom is fiddly; if reliable selection isn't achievable in the unit test, assert instead at the store+serialize level — call the exported `createCommentOnSelection` helper against a test editor with a set selection — and cover the full click flow in the Task 10 e2e. Note which you did.)

- [ ] **Step 2: Run → fails.**

- [ ] **Step 3: Implement.** Add a `HighlightButton` using the existing `MarkButton` pattern (it already supports `nodeType`): `<MarkButton label="Highlight" nodeType={KEYS.highlight}><Highlighter size={15} /></MarkButton>` (import `Highlighter` from `lucide-react`, `KEYS` from `platejs`). For comments, add a button whose handler runs:

```ts
import { nanoid } from 'nanoid';
import { CommentPlugin } from '@platejs/comment/react';
import { getCommentKey, getDraftCommentKey } from '@platejs/comment';
import { currentAuthor } from '../review/identity';
import { useReviewStore, addComment } from '../review/review-store';

// In the toolbar component (has access to `editor`):
function createCommentOnSelection(editor: PlateEditor) {
  editor.tf.comment.setDraft();
  const id = nanoid();
  const drafts = editor.getApi(CommentPlugin).comment.nodes({ at: [], isDraft: true });
  if (drafts.length === 0) return;
  editor.tf.withoutNormalizing(() => {
    for (const [, path] of drafts) {
      editor.tf.setNodes({ [getCommentKey(id)]: true }, { at: path, split: true });
      editor.tf.unsetNodes([getDraftCommentKey()], { at: path });
    }
  });
  const meta = addComment(useReviewStore.getState().getMeta(), id, { by: currentAuthor(), at: new Date().toISOString() });
  useReviewStore.getState().setMeta(meta);
  editor.tf.focus();
}
```

Wire a toolbar button calling `createCommentOnSelection(editor)`. Export `createCommentOnSelection` for the test. (The store `setMeta` triggers the Task 5 store-subscription save.)

- [ ] **Step 4: Run → passes;** editor suite + typecheck green.

- [ ] **Step 5: Manual verification.** In `bun run dev`: select text, click Highlight → `{==...==}` saved; select text, click Comment → `{==...==}{#c…}` + `comments:` endmatter saved (reload shows the mark). Report.

- [ ] **Step 6: Commit.**

```bash
git add ui/src/features/editor/PlateMarkdownEditor.tsx ui/src/features/editor/PlateMarkdownEditor.review.test.tsx
git commit -m "feat(review): toolbar actions to highlight and comment a selection

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

**Milestone: comments + highlights round-trip through the editor.** Tasks 7–10 add suggestions.

---

### Task 7: Suggesting-mode toggle

**Files:** Modify `ui/src/features/editor/PlateMarkdownEditor.tsx` (add a control); test.

Add a toggle (in the floating toolbar or the document header — match where the view/mode controls live) that flips `isSuggesting`. `currentUserId` is already set at mount (Task 5), satisfying the `withSuggestion` normalization gotcha.

- [ ] **Step 1: Failing test.** Render the editor, assert a "Suggest" toggle exists and that clicking it flips `editor.getOption(SuggestionPlugin, 'isSuggesting')` from `false` to `true`. (Use a test seam: expose the editor via a ref/callback, or assert the button's pressed state which reads `usePluginOption(SuggestionPlugin, 'isSuggesting')`.)

- [ ] **Step 2: Run → fails.**

- [ ] **Step 3: Implement** a `SuggestToggle` modeled on Potion's `suggestion-toolbar-button-app.tsx`:

```ts
import { SuggestionPlugin } from '@platejs/suggestion/react';
import { usePluginOption } from 'platejs/react';
import { PencilLine } from 'lucide-react';

function SuggestToggle() {
  const editor = useEditorRef();
  const isSuggesting = usePluginOption(SuggestionPlugin, 'isSuggesting');
  return (
    <button
      aria-label="Suggest edits"
      aria-pressed={isSuggesting}
      className={cn('inline-flex size-7 items-center justify-center rounded text-muted transition-colors hover:bg-well hover:text-body', isSuggesting && 'bg-well text-accent-ink')}
      onMouseDown={(e) => e.preventDefault()}
      onClick={() => editor.setOption(SuggestionPlugin, 'isSuggesting', !isSuggesting)}
      title={isSuggesting ? 'Stop suggesting' : 'Suggest edits'}
      type="button"
    >
      <PencilLine size={15} />
    </button>
  );
}
```

Place it in the toolbar.

- [ ] **Step 4: Run → passes;** typecheck.

- [ ] **Step 5: Manual verification.** `bun run dev`: enable Suggest, type → text appears as an insertion mark; delete → deletion mark; save → Markdown contains `{++…++}{#s…}` / `{--…--}{#s…}` + `suggestions:` endmatter with `by: user`. Reload → marks persist. Report.

- [ ] **Step 6: Commit.**

```bash
git add ui/src/features/editor/PlateMarkdownEditor.tsx ui/src/features/editor/PlateMarkdownEditor.review.test.tsx
git commit -m "feat(review): live Suggesting mode toggle

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: Resolve suggestions from the document (for accept/reject)

**Files:** Create `ui/src/features/review/resolve-suggestions.ts`, `ui/src/features/review/resolve-suggestions.test.ts`.

To accept/reject, Plate needs a `TResolvedSuggestion` (minimum `keyId` + `suggestionId`). Build them by walking the value's suggestion marks (transcribed from Plate's `toResolvedSuggestion`).

- [ ] **Step 1: Write the failing test** `resolve-suggestions.test.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { resolveSuggestions } from './resolve-suggestions';

const value = [
  { type: 'p', children: [
    { text: 'a' },
    { text: 'ins', suggestion: true, suggestion_s1: { id: 's1', type: 'insert', userId: 'user', createdAt: 0 } },
    { text: 'b' },
    { text: 'del', suggestion: true, suggestion_s2: { id: 's2', type: 'remove', userId: 'user', createdAt: 0 } },
  ] },
];

describe('resolveSuggestions', () => {
  it('returns one descriptor per suggestion id with keyId + suggestionId + type', () => {
    const out = resolveSuggestions(value).sort((a, b) => a.suggestionId.localeCompare(b.suggestionId));
    expect(out.map((s) => s.suggestionId)).toEqual(['s1', 's2']);
    expect(out[0]).toMatchObject({ suggestionId: 's1', keyId: 'suggestion_s1', type: 'insert', newText: 'ins' });
    expect(out[1]).toMatchObject({ suggestionId: 's2', keyId: 'suggestion_s2', type: 'remove', text: 'del' });
  });

  it('derives replace when an id has both insert and remove text', () => {
    const v = [{ type: 'p', children: [
      { text: 'old', suggestion: true, suggestion_s3: { id: 's3', type: 'remove', userId: 'user', createdAt: 0 } },
      { text: 'new', suggestion: true, suggestion_s3: { id: 's3', type: 'insert', userId: 'user', createdAt: 0 } },
    ] }];
    const [s] = resolveSuggestions(v);
    expect(s).toMatchObject({ suggestionId: 's3', type: 'replace', text: 'old', newText: 'new' });
  });
});
```

- [ ] **Step 2: Run → fails.**

- [ ] **Step 3: Implement** `ui/src/features/review/resolve-suggestions.ts`. Walk the value, group text by suggestion id, accumulate `newText` (insert) and `text` (remove), derive `type` (both→replace, newText→insert, text→remove), and build the `TResolvedSuggestion` fields Plate's transforms consume:

```ts
import { getSuggestionKey, keyId2SuggestionId, type TResolvedSuggestion } from '@platejs/suggestion';
import type { Descendant } from 'platejs';

interface Acc { newText: string; text: string; userId: string; createdAt: number; }

export function resolveSuggestions(value: Descendant[]): TResolvedSuggestion[] {
  const byId = new Map<string, Acc>();
  const visit = (node: Record<string, unknown>) => {
    const text = node.text;
    if (typeof text === 'string') {
      for (const key of Object.keys(node)) {
        if (!key.startsWith('suggestion_')) continue;
        const raw = node[key];
        if (typeof raw !== 'object' || raw === null) continue;
        const data: Record<string, unknown> = { ...raw };
        const id = data.id;
        const type = data.type;
        if (typeof id !== 'string') continue;
        const acc = byId.get(id) ?? { newText: '', text: '', userId: typeof data.userId === 'string' ? data.userId : 'user', createdAt: typeof data.createdAt === 'number' ? data.createdAt : 0 };
        if (type === 'insert') acc.newText += text;
        else if (type === 'remove') acc.text += text;
        byId.set(id, acc);
      }
    }
    const children = node.children;
    if (Array.isArray(children)) {
      for (const child of children) {
        if (typeof child === 'object' && child !== null) visit(child as Record<string, unknown>);
      }
    }
  };
  for (const node of value as Record<string, unknown>[]) visit(node);

  const out: TResolvedSuggestion[] = [];
  for (const [id, acc] of byId.entries()) {
    const base = { keyId: getSuggestionKey(id), suggestionId: keyId2SuggestionId(getSuggestionKey(id)), userId: acc.userId, createdAt: new Date(acc.createdAt) };
    if (acc.newText && acc.text) out.push({ ...base, type: 'replace', newText: acc.newText, text: acc.text });
    else if (acc.newText) out.push({ ...base, type: 'insert', newText: acc.newText });
    else if (acc.text) out.push({ ...base, type: 'remove', text: acc.text });
  }
  return out;
}
```

(Block-level/`update` suggestions are out of scope — v1 only produces inline insert/remove/replace per the design's "degrade to inline" decision.)

- [ ] **Step 4: Run → passes** (2 tests); typecheck. **Step 5: Commit.**

```bash
git add ui/src/features/review/resolve-suggestions.ts ui/src/features/review/resolve-suggestions.test.ts
git commit -m "feat(review): reconstruct resolved suggestions from document marks

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 9: Accept / reject a suggestion (editor commands)

**Files:** Modify `ui/src/features/editor/PlateMarkdownEditor.tsx` (a minimal per-suggestion accept/reject control); test.

Plan 3 builds the rail with per-card accept/reject; here we add minimal controls so the behavior exists and is testable. Add a small context action (e.g. when the selection is inside a suggestion, show Accept/Reject in the floating toolbar) that resolves the suggestions under the selection and applies accept/reject.

- [ ] **Step 1: Failing test** — exercise the command layer directly (DOM-free): build a test editor (via `createPlateEditor` with `reviewKit`) holding a value with one insert suggestion, call the exported `acceptSuggestionById(editor, id)`, assert the suggestion mark is gone and the inserted text remains; for a remove suggestion, assert reject keeps the text. (Wrap in `withoutSuggestions`.)

- [ ] **Step 2: Run → fails.**

- [ ] **Step 3: Implement** helpers in `PlateMarkdownEditor.tsx` (or a small `review/accept-reject.ts`):

```ts
import { acceptSuggestion, rejectSuggestion } from '@platejs/suggestion';
import { SuggestionPlugin } from '@platejs/suggestion/react';
import { resolveSuggestions } from '../review/resolve-suggestions';

export function acceptSuggestionById(editor: PlateEditor, id: string) {
  const desc = resolveSuggestions(editor.children as never).find((s) => s.suggestionId === id);
  if (!desc) return;
  editor.getApi(SuggestionPlugin).suggestion.withoutSuggestions(() => acceptSuggestion(editor, desc));
}
export function rejectSuggestionById(editor: PlateEditor, id: string) {
  const desc = resolveSuggestions(editor.children as never).find((s) => s.suggestionId === id);
  if (!desc) return;
  editor.getApi(SuggestionPlugin).suggestion.withoutSuggestions(() => rejectSuggestion(editor, desc));
}
```

Wire a minimal toolbar Accept/Reject pair shown when the selection sits inside a suggestion (detect via `editor.getApi(SuggestionPlugin).suggestion.nodeId(...)` or by checking the selected leaf for a `suggestion_*` key). After accept/reject, the suggestion mark disappears → the next save drops it from the Markdown (Plan 1's orphan-prune handles the endmatter).

- [ ] **Step 4: Run → passes;** full editor suite + typecheck green.

- [ ] **Step 5: Manual verification.** `bun run dev`: with a suggestion present, Accept applies it (text stays, mark gone, saved Markdown no longer has the `{++…++}`); Reject reverts it. Report.

- [ ] **Step 6: Commit.**

```bash
git add ui/src/features/editor/PlateMarkdownEditor.tsx ui/src/features/review/accept-reject.ts ui/src/features/editor/PlateMarkdownEditor.review.test.tsx
git commit -m "feat(review): accept/reject suggestions in the editor

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 10: End-to-end round-trip (Playwright)

**Files:** Create `ui/e2e/review-round-trip.spec.ts`.

A browser test proving the full loop, since several Task 5–9 behaviors are runtime/DOM-bound. Follow the existing Playwright config/patterns in `ui/` (`bun run test:e2e`).

- [ ] **Step 1: Write the e2e** that: loads the app on a document containing `{==here==}{>>fix this<<}{#c1}` + endmatter; asserts the commented text renders with the comment mark; selects a word and clicks Highlight, asserting the saved/reloaded document gains `{==word==}`; enables Suggest, types, and asserts the saved document gains `{++…++}{#s…}` + a `suggestions:` endmatter entry; accepts the suggestion and asserts it's applied and removed from the Markdown. Use stable selectors (`aria-label`/`data-testid` on the toolbar buttons added in Tasks 6–9 — add `data-testid`s as needed).

- [ ] **Step 2: Run → iterate to green.** `cd ui && bun run test:e2e --grep review`. (e2e tends to need selector/timing iteration; that's expected. Don't weaken assertions — they are the contract for the round-trip.)

- [ ] **Step 3: Commit.**

```bash
git add ui/e2e/review-round-trip.spec.ts ui/src/features/editor/PlateMarkdownEditor.tsx
git commit -m "test(review): e2e round-trip for comments, highlights, suggestions

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Done criteria (Plan 2)

- Opening a `.md` with review markup renders comment/highlight/suggestion marks; the discussion store hydrates from the endmatter.
- Toolbar actions create comments + highlights; live Suggesting mode records insert/delete/replace; accept/reject apply/revert.
- Every change (editor value OR store) saves back to Markdown through the Plan 1 codec, preserving CriticMarkup + endmatter; reload restores the marks.
- `currentUserId` is set before suggesting (no normalization loss).
- All new unit/integration tests + the e2e pass; `bun run typecheck` clean; no `as`/`any`/`!` in production code.

## Out of scope (Plan 3)

- The polished review rail (anchored cards, collision layout), comment composer + reply UI, suggestion cards with inline accept/reject, hover/active highlighting, the "Suggesting" status pill — all the Potion-derived presentation. Plan 2 ships minimal toolbar controls only.
- Block-level suggestion fidelity, the `<<}`-in-body endmatter fallback, and the inline-formatted-anchor limitation remain as documented in the design doc.

## Risks / notes

- **Plate version:** new plugins pinned to `52.0.11` to match `@platejs/*`; if the installed minor differs, align them and re-verify imports.
- **Integration tasks (5, 6, 7, 9) are runtime-bound:** their unit/component tests assert what jsdom can reach; the Task 10 e2e + the manual-verification steps are load-bearing for the rest. Don't skip the manual checks.
- **`withSuggestion` normalization:** `currentUserId` MUST be non-null before `isSuggesting` is enabled (handled in Task 5 mount effect) — verify it survives `resetPlateEditor` on external change.
- **Store-driven saves:** the Task 5 store subscription serializes `editor.children` on every store change; confirm it doesn't double-fire with `onValueChange` (the `lastSerializedRef` equality guard should absorb duplicates).
