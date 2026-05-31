# Review Markup Layer — Plan 3: Review UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the visible review UI on top of Plans 1–2: a comment/suggestion **rail** (document-ordered list of thread + suggestion cards), inline comment composer / replies / resolve, suggestion cards with accept/reject, active/hover highlight sync between rail and in-text marks, leaf styling, and a "Suggesting" status pill.

**Architecture:** The rail mounts **inside** `PlateMarkdownEditor` (it needs the live `editor` via `useEditorRef` and the `<Plate>` context). It reads review state from the existing Zustand `useReviewStore` (`buildThreads(meta)` for comment threads; `resolveSuggestions(editor.children)` for suggestion cards) and mutates via the existing pure reducers (`addReply`/`editComment`/`resolveComment`/`deleteComment`) + `acceptSuggestionById`/`rejectSuggestionById` (Plan 2). Because `PlateMarkdownEditor` already re-serializes on store change, all rail edits auto-save through the RFM codec — no new persistence wiring. Comment bodies are **Markdown strings** (textarea to compose/edit; rendered as text in view), not Potion's nested Plate editor.

**Active/hover sync:** `@platejs/comment@52.0.11` exposes NO `activeId`/`hoverId` plugin options, so this plan **owns active/hover state in `useReviewStore`** (`activeId`/`hoverId` + setters). Custom `CommentLeaf`/`SuggestionLeaf` render components (registered on the plugins) read this state to highlight in-text and set it on click/hover; rail cards read/set the same. One source of truth for both surfaces.

**Scope — v1 = list rail (NO margin-anchoring).** Cards render in a fixed right-side panel in document order, NOT vertically anchored to each comment's text with collision avoidance. Potion's `getCommentTop` + the four collision functions (`resolveOverlappingTop`/`updateActiveTop`/`updateActiveBelow`/`updateTopCommenting`) are **deferred to a follow-up** — they're the brittlest, most visual piece and not required to ship a functional review UI. Active card scrolls into view instead.

**Tech Stack:** TypeScript (ESM), Vitest + Testing Library, Playwright. React 19, PlateJS 52, `zustand`, `lucide-react`, Radix (`@radix-ui/react-dropdown-menu`), `cn()` from `lib/utils`, Tailwind v4 semantic tokens. **`ui/` uses bun** — `bunx vitest`, `bun run typecheck`, `bun run test:e2e`. (See [[quarry-ui-uses-bun]].)

**Builds on Plans 1–2** (merged on this branch): `ui/src/features/review/` (`rfm-types`, `review-store` with `buildThreads`/reducers, `resolve-suggestions`, `accept-reject`, `identity`) and `ui/src/features/editor/` (`review-kit`, `PlateMarkdownEditor` with the codec/store wiring, `createCommentOnSelection`, `SuggestToggle`, `SuggestionActions`). **Design:** `docs/superpowers/specs/2026-05-30-review-markup-layer-design.md`.

**Quarry conventions to match:** semantic tokens — surfaces `bg-canvas/panel/surface/raised/well`, text `text-ink/body/muted/faint`, lines `border-line/line-strong`, accent `bg-accent text-accent-ink text-on-accent`, unresolved/warn `bg-warn-tint text-warn-ink border-warn-line`, `text-danger`. Buttons are class strings (`ghostIconButton`-style, see `PlateMarkdownEditor.tsx`), menus via `@radix-ui/react-dropdown-menu` (imported `* as DropdownMenu`), icons `lucide-react`, `cn()` for class merging. No shadcn, no `date-fns`, no Avatar/time helpers (build them).

---

## File structure (Plan 3)

| File | Responsibility |
|---|---|
| `ui/src/features/review/suggestion-mark.ts` | (cleanup) shared `readSuggestionMark(leaf)` reader; used by the 4 existing walkers. |
| `ui/src/features/review/rfm-types.ts` | (cleanup) export `cloneMeta`. |
| `ui/src/features/review/review-store.ts` | add `activeId`/`hoverId` + `setActiveId`/`setHoverId`; reuse shared reader + `cloneMeta`. |
| `ui/src/features/review/format.ts` | `formatRelativeTime(iso)`, `initials(by)` utils. |
| `ui/src/features/review/ui/CommentThreadCard.tsx` | one comment thread: header, body (view/edit), replies, composer, resolve/delete. |
| `ui/src/features/review/ui/SuggestionCard.tsx` | one suggestion: add/delete/replace summary + accept/reject. |
| `ui/src/features/review/ui/ReviewRail.tsx` | the rail container: lists thread + suggestion cards (doc order), reads store + editor. |
| `ui/src/features/editor/review-leaves.tsx` | `CommentLeaf`/`SuggestionLeaf` render components (active/hover styling). |
| `ui/src/features/editor/review-kit.ts` | (modify) register the leaf render components. |
| `ui/src/features/editor/PlateMarkdownEditor.tsx` | (modify) mount `<ReviewRail/>` + the suggesting pill; reseed-via-serialize cleanup. |
| `ui/src/styles.css` | (modify) `.slate-comment` + suggestion `ins`/`del` styling. |
| Co-located `*.test.ts(x)` + `ui/tests/review-rail.spec.ts`. |

---

### Task 1: Cleanup carried from Plan 2 (shared mark reader + cloneMeta)

**Files:** Create `ui/src/features/review/suggestion-mark.ts` + test; Modify `ui/src/features/review/rfm-types.ts`, `review-store.ts`, `resolve-suggestions.ts`, `rfm-codec.ts`, `review-md-rules.ts`, `apply-critic-markup.ts`.

Four+ files independently walk `suggestion_<id>` leaf keys; `cloneMeta` is duplicated. Consolidate before adding more readers in the rail.

- [ ] **Step 1: Write the failing test** `ui/src/features/review/suggestion-mark.test.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { readSuggestionMark } from './suggestion-mark';

describe('readSuggestionMark', () => {
  it('reads the suggestion data object off a leaf', () => {
    expect(readSuggestionMark({ text: 'x', suggestion: true, suggestion_s1: { id: 's1', type: 'insert', userId: 'AI', createdAt: 5 } }))
      .toEqual({ id: 's1', type: 'insert', userId: 'AI', createdAt: 5 });
  });
  it('returns null when there is no suggestion key', () => {
    expect(readSuggestionMark({ text: 'x' })).toBeNull();
  });
  it('ignores a malformed suggestion value', () => {
    expect(readSuggestionMark({ text: 'x', suggestion_s2: { type: 'insert' } })).toBeNull();
  });
});
```

- [ ] **Step 2: Run → fails.** `cd ui && bunx vitest run src/features/review/suggestion-mark.test.ts`.

- [ ] **Step 3: Implement** `ui/src/features/review/suggestion-mark.ts` (cast-free):

```ts
export interface SuggestionMark {
  id: string;
  type: 'insert' | 'remove' | 'update';
  userId: string;
  createdAt: number;
}

/** Read the first `suggestion_<id>` data object off a leaf, or null. */
export function readSuggestionMark(leaf: Record<string, unknown>): SuggestionMark | null {
  for (const key of Object.keys(leaf)) {
    if (!key.startsWith('suggestion_')) continue;
    const raw = leaf[key];
    if (typeof raw !== 'object' || raw === null) continue;
    const data: Record<string, unknown> = { ...raw };
    const { id, type, userId, createdAt } = data;
    if (typeof id === 'string' && (type === 'insert' || type === 'remove' || type === 'update')) {
      return { id, type, userId: typeof userId === 'string' ? userId : 'user', createdAt: typeof createdAt === 'number' ? createdAt : 0 };
    }
  }
  return null;
}
```

- [ ] **Step 4: Export `cloneMeta` from `rfm-types.ts`.** Add:

```ts
export function cloneMeta(meta: ReviewMeta): ReviewMeta {
  return { comments: { ...meta.comments }, suggestions: { ...meta.suggestions } };
}
```

- [ ] **Step 5: Refactor the call sites to use the shared helpers.** Replace the inline `suggestion_*` key-walking in `review-store.ts` (`syncSuggestionsFromValue`), `resolve-suggestions.ts` (`resolveSuggestions`), and `rfm-codec.ts` (`liveIds`) to use `readSuggestionMark(node)`; replace `review-md-rules.ts`'s local `suggestionData` with it (note `readSuggestionMark` returns `type: insert|remove|update` — `review-md-rules` only emits insert/remove, so its `if (!data) return leaf.text` guard stays, and it treats `update` like... leave `update` unhandled there as today). Replace the open-coded `cloneMeta` in `apply-critic-markup.ts` and the local one in `review-store.ts` with the `rfm-types` export. **Do each refactor minimally and keep behavior identical** — the existing tests are the gate.

- [ ] **Step 6: Run the full review suite + typecheck.** `cd ui && bunx vitest run src/features/review && bun run typecheck`. All previously-green tests (≈54) MUST stay green; zero `as`/`any`/`!`. If any refactor changes behavior, revert that one site and report it.

- [ ] **Step 7: Commit.**

```bash
git add ui/src/features/review/suggestion-mark.ts ui/src/features/review/suggestion-mark.test.ts ui/src/features/review/rfm-types.ts ui/src/features/review/review-store.ts ui/src/features/review/resolve-suggestions.ts ui/src/features/review/rfm-codec.ts ui/src/features/review/review-md-rules.ts ui/src/features/review/apply-critic-markup.ts
git commit -m "refactor(review): shared suggestion-mark reader + cloneMeta

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Store active/hover state + display utils

**Files:** Modify `ui/src/features/review/review-store.ts` + test; Create `ui/src/features/review/format.ts` + test.

- [ ] **Step 1: Write failing tests.** Append to `review-store.test.ts`:

```ts
import { useReviewStore } from './review-store';

describe('review-store active/hover', () => {
  it('sets and clears activeId / hoverId', () => {
    useReviewStore.getState().setActiveId('c1');
    expect(useReviewStore.getState().activeId).toBe('c1');
    useReviewStore.getState().setHoverId('s1');
    expect(useReviewStore.getState().hoverId).toBe('s1');
    useReviewStore.getState().setActiveId(null);
    expect(useReviewStore.getState().activeId).toBeNull();
  });
});
```

Create `ui/src/features/review/format.test.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { initials } from './format';

describe('initials', () => {
  it('takes the first letter, uppercased', () => {
    expect(initials('user')).toBe('U');
    expect(initials('AI')).toBe('A');
    expect(initials('')).toBe('?');
  });
});
```

- [ ] **Step 2: Run → fails.**

- [ ] **Step 3: Implement.** In `review-store.ts`, extend the store state with `activeId: string | null`, `hoverId: string | null` (both default `null`) and `setActiveId(id)`, `setHoverId(id)` actions. Create `ui/src/features/review/format.ts`:

```ts
export function initials(by: string): string {
  return by.trim().charAt(0).toUpperCase() || '?';
}

const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' });

/** "3 minutes ago" style; falls back to the raw string if unparseable. */
export function formatRelativeTime(iso: string): string {
  const ms = Date.parse(iso);
  if (Number.isNaN(ms)) return iso;
  const diffSec = Math.round((ms - Date.now()) / 1000);
  const abs = Math.abs(diffSec);
  if (abs < 60) return rtf.format(Math.round(diffSec), 'second');
  if (abs < 3600) return rtf.format(Math.round(diffSec / 60), 'minute');
  if (abs < 86400) return rtf.format(Math.round(diffSec / 3600), 'hour');
  return rtf.format(Math.round(diffSec / 86400), 'day');
}
```

(Do NOT unit-test `formatRelativeTime`'s exact output — it depends on `Date.now()`; the `initials` test + the e2e cover display. If you want a deterministic test, inject `now` as a param — optional.)

- [ ] **Step 4: Run → passes;** `cd ui && bunx vitest run src/features/review && bun run typecheck`.

- [ ] **Step 5: Commit.**

```bash
git add ui/src/features/review/review-store.ts ui/src/features/review/review-store.test.ts ui/src/features/review/format.ts ui/src/features/review/format.test.ts
git commit -m "feat(review): store active/hover state + display utils

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Leaf styling + render components (active/hover)

**Files:** Create `ui/src/features/editor/review-leaves.tsx`; Modify `ui/src/features/editor/review-kit.ts`, `ui/src/styles.css`.

- [ ] **Step 1: Write a failing component test** `ui/src/features/editor/review-leaves.test.tsx`. Render a tiny Plate editor whose value has a `comment_c1` leaf, with the `CommentLeaf` registered; assert the commented text renders inside an element carrying `data-comment-id="c1"` and that when the store `activeId === 'c1'` it gets an `data-active="true"` attribute. (If a full editor render is heavy, render `CommentLeaf` directly with mocked `PlateLeaf` props — but prefer the real editor for fidelity. Inspect the real DOM and assert on the actual attributes you emit.) Keep the assertion meaningful.

- [ ] **Step 2: Run → fails.**

- [ ] **Step 3: Implement** `ui/src/features/editor/review-leaves.tsx`. `CommentLeaf` reads the comment id off the leaf (the `comment_<id>` key, excluding `comment_draft`) and the active/hover ids from the store; renders `PlateLeaf` with `data-comment-id`, `data-active`, `data-hover`, an `onClick` that `setActiveId(id)`, and `onMouseEnter/Leave` that set/clear `hoverId`. Style with quarry's `warn` (unresolved highlight) tokens, intensifying on active/hover:

```tsx
import { PlateLeaf, type PlateLeafProps } from 'platejs/react';
import { useReviewStore } from '../review/review-store';
import { readSuggestionMark } from '../review/suggestion-mark';
import { cn } from '../../lib/utils';

function commentIdOf(leaf: Record<string, unknown>): string | null {
  for (const key of Object.keys(leaf)) {
    if (key.startsWith('comment_') && key !== 'comment_draft' && leaf[key] === true) return key.slice('comment_'.length);
  }
  return null;
}

export function CommentLeaf(props: PlateLeafProps) {
  const id = commentIdOf(props.leaf);
  const activeId = useReviewStore((s) => s.activeId);
  const hoverId = useReviewStore((s) => s.hoverId);
  const setActiveId = useReviewStore((s) => s.setActiveId);
  const setHoverId = useReviewStore((s) => s.setHoverId);
  const isActive = !!id && activeId === id;
  const isHover = !!id && hoverId === id;
  return (
    <PlateLeaf
      {...props}
      attributes={{
        ...props.attributes,
        'data-comment-id': id ?? undefined,
        'data-active': isActive ? 'true' : 'false',
        'data-hover': isHover ? 'true' : 'false',
        onClick: () => id && setActiveId(id),
        onMouseEnter: () => id && setHoverId(id),
        onMouseLeave: () => setHoverId(null),
      }}
      className={cn(
        'border-b-2 border-warn-line bg-warn-tint transition-colors',
        (isActive || isHover) && 'border-warn-ink',
      )}
    />
  );
}

export function SuggestionLeaf(props: PlateLeafProps) {
  const data = readSuggestionMark(props.leaf);
  const activeId = useReviewStore((s) => s.activeId);
  const setActiveId = useReviewStore((s) => s.setActiveId);
  const setHoverId = useReviewStore((s) => s.setHoverId);
  const id = data?.id ?? null;
  const isActive = !!id && activeId === id;
  const className =
    data?.type === 'remove'
      ? cn('text-danger line-through decoration-danger/60', isActive && 'bg-danger/10')
      : cn('text-accent-ink underline decoration-accent-line', isActive && 'bg-accent-tint');
  return (
    <PlateLeaf
      {...props}
      attributes={{
        ...props.attributes,
        'data-suggestion-id': id ?? undefined,
        onClick: () => id && setActiveId(id),
        onMouseEnter: () => id && setHoverId(id),
        onMouseLeave: () => setHoverId(null),
      }}
      className={className}
    />
  );
}
```

Register them in `review-kit.ts`: `CommentPlugin.configure({ shortcuts: {...}, node: { component: CommentLeaf } })` and `SuggestionPlugin.configure({ node: { component: SuggestionLeaf } })`. (Confirm the exact `node` config shape against the installed Plate version — earlier review-kit work showed `.configure` accepts `node`; if the render-component key differs, inspect `@platejs/comment`'s `CommentPlugin` config type and adjust.) Add the `.slate-comment` base rule to `styles.css` only if needed as a fallback (the component className now drives styling).

- [ ] **Step 4: Run → passes;** `cd ui && bunx vitest run src/features/editor && bun run typecheck`. Manually confirm in `bun run dev` that commented text shows the warn underline and suggestions show insert(accent)/delete(strikethrough) styling; clicking sets active.

- [ ] **Step 5: Commit.**

```bash
git add ui/src/features/editor/review-leaves.tsx ui/src/features/editor/review-leaves.test.tsx ui/src/features/editor/review-kit.ts ui/src/styles.css
git commit -m "feat(review): comment/suggestion leaf rendering with active/hover

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Comment thread card + composer

**Files:** Create `ui/src/features/review/ui/CommentThreadCard.tsx` + test.

A card for one `ReviewThread` (from `buildThreads`): header (initials avatar, `by`, relative time, resolved badge), body, replies (each with header + body), a reply composer (textarea + submit), and a resolve + delete action menu. Bodies are Markdown strings rendered as plain text in v1 (no markdown rendering inside cards yet — note as a follow-up). All mutations go through the store reducers + `setMeta` (which auto-saves).

- [ ] **Step 1: Write a failing test** `CommentThreadCard.test.tsx`. Render a card for a thread with one root + one reply; assert the author, body text, and reply body render; type into the reply textarea, submit, and assert `useReviewStore.getState().getMeta().comments` gained a reply entry with `re` = the root id and `by: 'user'`. Reset the store at test start. Assert resolve sets `status: 'resolved'`. Keep assertions meaningful.

- [ ] **Step 2: Run → fails.**

- [ ] **Step 3: Implement** `CommentThreadCard.tsx`. Props: `{ thread: ReviewThread }`. Use `useReviewStore` + `addReply`/`resolveComment`/`deleteComment`/`editComment` + `nanoid` for reply ids + `currentAuthor()` + `formatRelativeTime`/`initials`. On reply submit: `setMeta(addReply(getMeta(), nanoid(), { parentId: thread.id, body, by: currentAuthor(), at: new Date().toISOString() }))`. Resolve: `setMeta(resolveComment(getMeta(), thread.id, undefined))`. Delete: `setMeta(deleteComment(getMeta(), thread.id))`. Style with quarry tokens (card: `rounded-lg border border-line bg-raised p-3`; unresolved accent via `border-warn-line` when `!status`; resolved shows a muted "Resolved" badge). A Radix `DropdownMenu` for edit/delete (model on `TurnIntoButton`/`BlockActionsMenu` in `PlateMarkdownEditor.tsx`). The card sets `setActiveId(thread.id)` on click and reads `activeId` to show an active ring (`ring-2 ring-accent-ring`). Composer: a `<textarea>` styled like `App.tsx`'s manual-merge textarea; Enter (without shift) submits; submit disabled when empty.

- [ ] **Step 4: Run → passes;** `cd ui && bunx vitest run src/features/review && bun run typecheck`.

- [ ] **Step 5: Commit.**

```bash
git add ui/src/features/review/ui/CommentThreadCard.tsx ui/src/features/review/ui/CommentThreadCard.test.tsx
git commit -m "feat(review): comment thread card with replies, resolve, delete

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Suggestion card

**Files:** Create `ui/src/features/review/ui/SuggestionCard.tsx` + test.

A card for one resolved suggestion (`TResolvedSuggestion` from `resolveSuggestions`): header (initials, `by`, time), a summary line (Add/Delete/Replace + the changed text), and Accept/Reject buttons.

- [ ] **Step 1: Write a failing test** `SuggestionCard.test.tsx`. Render a card for an `insert` suggestion `{ suggestionId:'s1', keyId:'suggestion_s1', type:'insert', newText:'more', userId:'user', createdAt: new Date(0) }` with `onAccept`/`onReject` spies; assert the summary shows "Add" + "more"; click Accept → `onAccept('s1')` called; click Reject → `onReject('s1')`. For a `replace`, assert it shows old + new text. Keep assertions meaningful.

- [ ] **Step 2: Run → fails.**

- [ ] **Step 3: Implement** `SuggestionCard.tsx`. Props: `{ suggestion: TResolvedSuggestion; onAccept: (id: string) => void; onReject: (id: string) => void }`. Summary by `type`: `insert` → label "Add" + `newText` (accent); `remove` → "Delete" + `text` (danger strikethrough); `replace` → "Replace" + `text` (danger) "→" `newText` (accent); `update` → "Format change". Accept button (Check icon, `data-testid="rail-accept"`) calls `onAccept(suggestion.suggestionId)`; Reject (X icon, `data-testid="rail-reject"`) calls `onReject(...)`. Card click sets `setActiveId(suggestionId)`; active ring as in Task 4. Quarry tokens; lucide `Check`/`X`.

- [ ] **Step 4: Run → passes;** `cd ui && bunx vitest run src/features/review && bun run typecheck`.

- [ ] **Step 5: Commit.**

```bash
git add ui/src/features/review/ui/SuggestionCard.tsx ui/src/features/review/ui/SuggestionCard.test.tsx
git commit -m "feat(review): suggestion card with accept/reject

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: The rail container + mount

**Files:** Create `ui/src/features/review/ui/ReviewRail.tsx` + test; Modify `ui/src/features/editor/PlateMarkdownEditor.tsx`.

- [ ] **Step 1: Write a failing test** `ReviewRail.test.tsx`. Render `<ReviewRail editor={testEditor} />` (build a Plate editor with `reviewKit` whose value has a `comment_c1` mark + a `suggestion_s1` insert mark, and a store hydrated with `comments.c1` + a matching suggestion). Assert the rail renders one comment card (author/body) and one suggestion card ("Add"). Assert an empty state renders ("No comments or suggestions") when the store/value have none. Keep assertions meaningful.

- [ ] **Step 2: Run → fails.**

- [ ] **Step 3: Implement** `ReviewRail.tsx`. Props: `{ editor: PlateEditor }`. Read `const meta = useReviewStore(s => s.meta)`; `const threads = useMemo(() => buildThreads(meta), [meta])`; `const suggestions = useEditorSelector((ed) => resolveSuggestions(ed.children), [])` (recomputes on editor change). Render comment `CommentThreadCard`s + `SuggestionCard`s (suggestion accept/reject call `acceptSuggestionById(editor, id)` / `rejectSuggestionById(editor, id)`). v1 order: comments then suggestions (document-anchored ordering is the deferred follow-up). Container: `<aside aria-label="Review" className="flex h-full w-80 shrink-0 flex-col gap-2 overflow-auto border-l border-line bg-surface p-3">`; empty state in `text-muted text-sm`. Hide the rail (render null) when there are zero threads AND zero suggestions, so it doesn't take space on docs with no review activity. (Optional: a small count header.)

- [ ] **Step 4: Mount the rail in `PlateMarkdownEditor.tsx`.** The rail must be inside `<Plate>` (for `useEditorRef`/store/editor). Wrap the `PlateContent` and the rail in a flex row inside `<Plate>`: a scrollable editor column (`flex-1`) + `<ReviewRail editor={editor} />`. Preserve the centered-column padding on `PlateContent`. Pass the `editor` from `useEditorRef()` (the toolbar already uses it) or the `editor` instance in scope. Verify the existing layout/tests still pass.

- [ ] **Step 5: Run → passes;** `cd ui && bunx vitest run src/features/editor src/features/review && bun run typecheck`. Manual: `bun run dev` — a doc with comments/suggestions shows the rail with cards; reply/resolve/accept/reject work and persist (reload). Report observations or gap.

- [ ] **Step 6: Commit.**

```bash
git add ui/src/features/review/ui/ReviewRail.tsx ui/src/features/review/ui/ReviewRail.test.tsx ui/src/features/editor/PlateMarkdownEditor.tsx
git commit -m "feat(review): review rail listing comment + suggestion cards

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Active/hover highlight sync

**Files:** Modify `ui/src/features/review/ui/ReviewRail.tsx`, `CommentThreadCard.tsx`, `SuggestionCard.tsx`; test.

Wire the two-way highlight: clicking/hovering an in-text mark highlights its rail card and vice-versa (already half-done — leaves set store active/hover in Task 3; cards set active on click in Tasks 4–5). This task closes the loop: cards read `activeId`/`hoverId` to show active/hover styling, and the active card scrolls into view (the v1 substitute for margin-anchoring).

- [ ] **Step 1: Write a failing test.** In `ReviewRail.test.tsx` (or the card tests): set `useReviewStore.getState().setActiveId('c1')` and assert the matching card has the active treatment (e.g. `data-active="true"` / the active ring class). Set `setHoverId` and assert hover treatment. Keep meaningful.

- [ ] **Step 2: Run → fails** (cards don't yet read active/hover for styling, or lack the `data-active` hook).

- [ ] **Step 3: Implement.** In `CommentThreadCard`/`SuggestionCard`: read `activeId`/`hoverId` from the store; add `data-active`/`data-hover` attributes + active-ring/hover-bg classes (`ring-2 ring-accent-ring` / `bg-well`). Add a `useEffect` that scrolls the card into view (`ref.current?.scrollIntoView({ block: 'nearest' })`) when it becomes active. On card `onMouseEnter/Leave`, set/clear `hoverId` (so hovering a card highlights the in-text mark via the Task 3 leaves).

- [ ] **Step 4: Run → passes;** editor+review suites + typecheck green. Manual: hovering a card highlights the text and vice-versa; clicking a mark scrolls its card into view. Report.

- [ ] **Step 5: Commit.**

```bash
git add ui/src/features/review/ui/ReviewRail.tsx ui/src/features/review/ui/CommentThreadCard.tsx ui/src/features/review/ui/SuggestionCard.tsx ui/src/features/review/ui/ReviewRail.test.tsx
git commit -m "feat(review): two-way active/hover highlight sync (rail <-> marks)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: Suggesting status pill + reseed cleanup

**Files:** Modify `ui/src/features/editor/PlateMarkdownEditor.tsx`; test.

- [ ] **Step 1: Failing test.** Assert that when `isSuggesting` is on, a "Suggesting" pill (`data-testid="suggesting-pill"`) renders, and clicking it sets `isSuggesting` false. (Use the same option-flip test seam as the Task 7/P2.7 toggle test.)

- [ ] **Step 2: Run → fails.**

- [ ] **Step 3: Implement** a `SuggestingPill` modeled on Potion's `document-suggesting.tsx`: reads `usePluginOption(SuggestionPlugin, 'isSuggesting')`; returns null when off; otherwise a pill (`PencilLine` + "Suggesting" + `X`, `data-testid="suggesting-pill"`, `text-accent-ink bg-accent-tint` styling) whose click does `editor.setOption(SuggestionPlugin, 'isSuggesting', false)`. Place it in a stable spot near the editor top (e.g. above `PlateContent`, or in the editor header area). **Also fold in the Plan-2 cleanup:** in the external-load `useEffect`, change the reseed line to `lastSerializedRef.current = serialize(value as PlateValue)` (self-consistent with the save paths), and add a one-line comment near `useReviewStore` noting the single-editor assumption.

- [ ] **Step 4: Run → passes;** editor suite + typecheck green. Manual: toggle suggesting → pill appears; click pill → suggesting off. Report.

- [ ] **Step 5: Commit.**

```bash
git add ui/src/features/editor/PlateMarkdownEditor.tsx
git commit -m "feat(review): Suggesting status pill + reseed self-consistency

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 9: End-to-end (rail interactions)

**Files:** Create `ui/tests/review-rail.spec.ts` (mirror `ui/tests/review-round-trip.spec.ts` + `workspace.spec.ts` harness).

- [ ] **Step 1: Write the e2e.** Load a doc with a comment (`{==here==}{>>fix this<<}{#c1}` + endmatter) and assert: (a) the rail shows a card with "fix this" and author; (b) typing a reply in the card's composer + submit persists a reply in the saved Markdown endmatter (`re: c1` + the reply body); (c) clicking Resolve persists `status: resolved`; (d) create a live suggestion (toggle Suggest, type), assert a suggestion card appears, click the rail Accept (`data-testid="rail-accept"`), assert the suggestion is applied and dropped from the saved Markdown; (e) hovering a rail card adds the in-text mark's active/hover treatment (assert the `[data-comment-id]` element's `data-hover`/class). Use stable `data-testid`/aria hooks. Don't weaken assertions.

- [ ] **Step 2: Run → iterate to green.** `cd ui && bun run test:e2e -- review-rail`. If browsers can't run in-sandbox, deliver a correct type-checking spec + a DONE_WITH_CONCERNS note (per the Plan 2 Task 10 precedent), don't fake green.

- [ ] **Step 3: Commit.**

```bash
git add ui/tests/review-rail.spec.ts
git commit -m "test(review): e2e for the review rail (reply, resolve, accept, hover)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Done criteria (Plan 3)

- A rail lists comment threads + suggestion cards for the open document; empty/hidden when there's no review activity.
- Comment cards: author, relative time, body, replies, reply composer, resolve, delete — all persisting through the codec on change.
- Suggestion cards: add/delete/replace summary + accept/reject (applying via the Plan 2 command layer).
- Two-way active/hover highlight sync between rail cards and in-text marks; active card scrolls into view.
- Leaf styling for comments (warn) + suggestions (accent insert / danger strikethrough delete).
- A "Suggesting" status pill while suggesting mode is on.
- Plan-2 cleanups folded in (shared mark reader, `cloneMeta` export, reseed self-consistency).
- All unit/integration tests + the e2e pass; `bun run typecheck` clean; no `as`/`any`/`!` in production.

## Out of scope / deferred (a possible Plan 4)

- **Margin-anchored cards + collision layout** (Potion's `getCommentTop` + `resolveOverlappingTop`/`updateActiveTop`/`updateActiveBelow`/`updateTopCommenting`). v1 is a document-ordered list; active card scrolls into view.
- **Markdown rendering inside comment bodies** (v1 renders bodies as plain text; `lineWidth: 0` YAML folding for long bodies in `serializeReviewMeta` should also land when rich bodies arrive).
- **Multi-editor** support (the store is a global singleton — fine while quarry mounts one editor).
- Block-level suggestion fidelity + the `<<}`-in-body endmatter fallback + inline-formatted-anchor handling (documented limitations from Plans 1–2).

## Risks / notes

- **`node` render-component config:** confirm the exact `CommentPlugin.configure({ node: { component } })` shape in the installed Plate 52; adjust if the key differs.
- **Rail mount must be inside `<Plate>`** (needs `editor` + store + Plate context). Don't mount it in `MarkdownEditor` (outside `<Plate>`).
- **Integration tasks (3, 6, 7, 8) are runtime-bound:** unit/component tests assert what jsdom reaches; the Task 9 e2e + manual steps are load-bearing. Don't skip the manual checks.
- **Active/hover is store-owned** (not Plate plugin options, which 52.0.11 lacks for comments) — keep the leaves and cards reading the same `useReviewStore` source.
