# Review Markup Layer — Plan 1: RFM Codec Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a pure, bidirectional codec that translates between quarry's Markdown (Roughdraft Flavored Markdown: CriticMarkup body + YAML endmatter) and the Plate editor's in-memory representation (comment/suggestion/highlight leaf marks + a review-metadata object).

**Architecture:** One chokepoint module, `rfm-codec.ts`, exposes `markdownToReview(md) → {value, meta}` and `reviewToMarkdown(value, meta) → md`. It composes small, single-purpose helpers: endmatter split/serialize, a Plate→CriticMarkup serialize ruleset, a substitution-collapse string pass, and a CriticMarkup→Plate value transform. No editor, store, UI, or backend code — this layer is testable in isolation.

**Tech Stack:** TypeScript (ESM), Vitest, `platejs` + `@platejs/markdown` (already present), `remark-gfm` (present), `yaml` + `nanoid` (new). Tests run from `ui/`.

**Reference design:** `docs/superpowers/specs/2026-05-30-review-markup-layer-design.md`.

**Planning note (refinement of the spec's module list):** the spec named a `remark-criticmarkup.ts` remark plugin for parsing. During planning we chose a simpler, more self-contained mechanism that avoids depending on Plate's internal mdast→rule routing: CriticMarkup survives `remark` as literal text, so **serialize** emits CriticMarkup via Plate `MdRules` + a string collapse pass, and **deserialize** is a pure `applyCriticMarkup` transform over the Plate value after `markdown.deserialize`. Same on-disk format; cleaner seam. v1 scans CriticMarkup spans **within a single text leaf** (the common, plain-text case); spans wrapping inline-formatted content are a documented v1 limitation (Task 6).

---

## File structure (Plan 1)

| File | Responsibility |
|---|---|
| `ui/src/features/review/rfm-types.ts` | TS model: `ReviewMetaEntry`, `ReviewMeta`, helpers. Types only. |
| `ui/src/features/review/endmatter.ts` | `splitEndmatter(md)` (+ guard), `serializeReviewMeta(meta)` (deterministic). |
| `ui/src/features/review/review-md-rules.ts` | `reviewMdRules(meta)` → Plate `MdRules` serializing comment/suggestion/highlight marks → CriticMarkup. |
| `ui/src/features/review/collapse-substitutions.ts` | `collapseSubstitutions(md)` — adjacent del+ins sharing an id → `{~~old~>new~~}`. |
| `ui/src/features/review/apply-critic-markup.ts` | `applyCriticMarkup(value, meta)` — CriticMarkup text in leaves → marked leaves; resolves/synthesizes ids. |
| `ui/src/features/review/rfm-codec.ts` | `markdownToReview`, `reviewToMarkdown` orchestrators. |
| Each `*.ts` above (except types) | Co-located `*.test.ts`. |

All review IDs are **nanoid** (collision-resistant, collab-agnostic). The Plate mark id = the `{#id}` ref = the endmatter key (one id space).

---

### Task 1: Dependencies + review model types

**Files:**
- Modify: `ui/package.json` (add deps)
- Create: `ui/src/features/review/rfm-types.ts`
- Test: `ui/src/features/review/rfm-types.test.ts`

- [ ] **Step 1: Add dependencies**

`yaml` and `nanoid` are long-established packages (well past the 24h-age policy); no postinstall scripts to review. Run from `ui/`:

```bash
cd ui && pnpm add yaml nanoid
```

Expected: `package.json` gains `"yaml"` and `"nanoid"` under `dependencies`; lockfile updates.

- [ ] **Step 2: Write the failing test**

```ts
// ui/src/features/review/rfm-types.test.ts
import { describe, expect, it } from 'vitest';
import { emptyReviewMeta, isEmptyReviewMeta } from './rfm-types';

describe('rfm-types', () => {
  it('emptyReviewMeta has empty comment and suggestion maps', () => {
    expect(emptyReviewMeta()).toEqual({ comments: {}, suggestions: {} });
  });

  it('isEmptyReviewMeta is true only when both maps are empty', () => {
    expect(isEmptyReviewMeta(emptyReviewMeta())).toBe(true);
    expect(isEmptyReviewMeta({ comments: { c1: { by: 'user', at: 'x' } }, suggestions: {} })).toBe(false);
    expect(isEmptyReviewMeta({ comments: {}, suggestions: { s1: { by: 'AI', at: 'x' } } })).toBe(false);
  });
});
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cd ui && pnpm exec vitest run src/features/review/rfm-types.test.ts`
Expected: FAIL — `Failed to resolve import './rfm-types'`.

- [ ] **Step 4: Write minimal implementation**

```ts
// ui/src/features/review/rfm-types.ts

/** Metadata for one review item, stored in YAML endmatter, keyed by id. */
export interface ReviewMetaEntry {
  /** Free-form author label: "user", "AI", or an agent name. */
  by: string;
  /** ISO 8601 timestamp. */
  at: string;
  /** Markdown body — used by replies and by comments not stored inline. */
  body?: string;
  /** Parent id (this entry is a reply to that id). */
  re?: string;
  /** Review state. */
  status?: 'resolved';
  /** Optional resolution summary. */
  resolved?: string;
}

/** The parsed review endmatter: two id-keyed maps. */
export interface ReviewMeta {
  comments: Record<string, ReviewMetaEntry>;
  suggestions: Record<string, ReviewMetaEntry>;
}

export function emptyReviewMeta(): ReviewMeta {
  return { comments: {}, suggestions: {} };
}

export function isEmptyReviewMeta(meta: ReviewMeta): boolean {
  return Object.keys(meta.comments).length === 0 && Object.keys(meta.suggestions).length === 0;
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cd ui && pnpm exec vitest run src/features/review/rfm-types.test.ts`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add ui/package.json ui/pnpm-lock.yaml ui/src/features/review/rfm-types.ts ui/src/features/review/rfm-types.test.ts
git commit -m "feat(review): add RFM review-metadata types and deps

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Split the YAML endmatter (with the false-endmatter guard)

**Files:**
- Create: `ui/src/features/review/endmatter.ts`
- Test: `ui/src/features/review/endmatter.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
// ui/src/features/review/endmatter.test.ts
import { describe, expect, it } from 'vitest';
import { splitEndmatter } from './endmatter';

describe('splitEndmatter', () => {
  it('returns null meta when there is no trailing endmatter', () => {
    const md = '# Title\n\nA paragraph.\n';
    expect(splitEndmatter(md)).toEqual({ body: md, meta: null });
  });

  it('splits a trailing review endmatter block with a comments map', () => {
    const md = 'Hello {==world==}{>>note<<}{#c1}.\n\n---\ncomments:\n  c1:\n    by: user\n    at: "2026-04-28T12:00:00.000Z"\n';
    const result = splitEndmatter(md);
    expect(result.body).toBe('Hello {==world==}{>>note<<}{#c1}.');
    expect(result.meta).toEqual({
      comments: { c1: { by: 'user', at: '2026-04-28T12:00:00.000Z' } },
      suggestions: {},
    });
  });

  it('does NOT treat an ordinary trailing --- + YAML as review endmatter', () => {
    const md = '# Notes\n\nSome prose.\n\n---\ntitle: My Doc\nauthor: Jane\n';
    expect(splitEndmatter(md)).toEqual({ body: md, meta: null });
  });

  it('uses only the final --- block as endmatter', () => {
    const md = 'Intro.\n\n---\n\nMore prose with a divider above.\n\n---\nsuggestions:\n  s1:\n    by: AI\n    at: "2026-04-28T12:00:00.000Z"\n';
    const result = splitEndmatter(md);
    expect(result.body).toBe('Intro.\n\n---\n\nMore prose with a divider above.');
    expect(result.meta?.suggestions.s1).toEqual({ by: 'AI', at: '2026-04-28T12:00:00.000Z' });
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd ui && pnpm exec vitest run src/features/review/endmatter.test.ts`
Expected: FAIL — `Failed to resolve import './endmatter'`.

- [ ] **Step 3: Write minimal implementation**

```ts
// ui/src/features/review/endmatter.ts
import { parse as parseYaml } from 'yaml';
import { emptyReviewMeta, type ReviewMeta, type ReviewMetaEntry } from './rfm-types';

const ENDMATTER_DELIMITER = /\n---[ \t]*\r?\n/g;

export interface SplitDocument {
  /** Document body with the review endmatter removed (trailing whitespace trimmed). */
  body: string;
  /** Parsed review metadata, or null when there is no review endmatter. */
  meta: ReviewMeta | null;
}

/**
 * Split a trailing YAML endmatter block off the document. Only the FINAL
 * `\n---\n` block counts, and it is treated as review endmatter only when it
 * parses to an object with a `comments` or `suggestions` mapping. Otherwise the
 * whole input is returned as body (ordinary prose ending in `---` is safe).
 */
export function splitEndmatter(markdown: string): SplitDocument {
  const matches = [...markdown.matchAll(ENDMATTER_DELIMITER)];
  const last = matches.at(-1);
  if (!last || last.index === undefined) return { body: markdown, meta: null };

  const delimiterEnd = last.index + last[0].length;
  const yamlText = markdown.slice(delimiterEnd);

  let parsed: unknown;
  try {
    parsed = parseYaml(yamlText);
  } catch {
    return { body: markdown, meta: null };
  }
  if (!isReviewObject(parsed)) return { body: markdown, meta: null };

  const meta = toReviewMeta(parsed);
  const body = markdown.slice(0, last.index).replace(/\s+$/, '');
  return { body, meta };
}

function isReviewObject(value: unknown): value is Record<string, unknown> {
  return (
    typeof value === 'object' &&
    value !== null &&
    !Array.isArray(value) &&
    ('comments' in value || 'suggestions' in value)
  );
}

function toReviewMeta(parsed: Record<string, unknown>): ReviewMeta {
  const meta = emptyReviewMeta();
  meta.comments = toEntryMap(parsed.comments);
  meta.suggestions = toEntryMap(parsed.suggestions);
  return meta;
}

function toEntryMap(value: unknown): Record<string, ReviewMetaEntry> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return {};
  const out: Record<string, ReviewMetaEntry> = {};
  for (const [id, raw] of Object.entries(value as Record<string, unknown>)) {
    if (typeof raw !== 'object' || raw === null) continue;
    const entry = raw as Record<string, unknown>;
    const by = typeof entry.by === 'string' ? entry.by : 'unknown';
    const at = typeof entry.at === 'string' ? entry.at : '';
    const next: ReviewMetaEntry = { by, at };
    if (typeof entry.body === 'string') next.body = entry.body;
    if (typeof entry.re === 'string') next.re = entry.re;
    if (entry.status === 'resolved') next.status = 'resolved';
    if (typeof entry.resolved === 'string') next.resolved = entry.resolved;
    out[id] = next;
  }
  return out;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd ui && pnpm exec vitest run src/features/review/endmatter.test.ts`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add ui/src/features/review/endmatter.ts ui/src/features/review/endmatter.test.ts
git commit -m "feat(review): split review YAML endmatter with false-endmatter guard

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Serialize review metadata deterministically

**Files:**
- Modify: `ui/src/features/review/endmatter.ts`
- Test: `ui/src/features/review/endmatter.test.ts`

- [ ] **Step 1: Write the failing test**

Append to `ui/src/features/review/endmatter.test.ts`:

```ts
import { serializeReviewMeta } from './endmatter';
import { emptyReviewMeta } from './rfm-types';

describe('serializeReviewMeta', () => {
  it('returns an empty string when there is no review data', () => {
    expect(serializeReviewMeta(emptyReviewMeta())).toBe('');
  });

  it('emits comments and suggestions with deterministic, sorted keys', () => {
    const yaml = serializeReviewMeta({
      comments: {
        c2: { body: 'reply', by: 'AI', at: '2026-04-28T12:05:00.000Z', re: 'c1' },
        c1: { by: 'user', at: '2026-04-28T12:00:00.000Z' },
      },
      suggestions: { s1: { by: 'AI', at: '2026-04-28T12:10:00.000Z' } },
    });
    expect(yaml).toBe(
      [
        'comments:',
        '  c1:',
        '    at: "2026-04-28T12:00:00.000Z"',
        '    by: user',
        '  c2:',
        '    at: "2026-04-28T12:05:00.000Z"',
        '    body: reply',
        '    by: AI',
        '    re: c1',
        'suggestions:',
        '  s1:',
        '    at: "2026-04-28T12:10:00.000Z"',
        '    by: AI',
        '',
      ].join('\n')
    );
  });

  it('is idempotent: serialize is stable across repeated calls', () => {
    const meta = { comments: { c1: { by: 'user', at: '2026-04-28T12:00:00.000Z' } }, suggestions: {} };
    expect(serializeReviewMeta(meta)).toBe(serializeReviewMeta(meta));
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd ui && pnpm exec vitest run src/features/review/endmatter.test.ts -t serializeReviewMeta`
Expected: FAIL — `serializeReviewMeta is not a function`.

- [ ] **Step 3: Write minimal implementation**

Add to the imports at the top of `ui/src/features/review/endmatter.ts`:

```ts
import { parse as parseYaml, stringify as stringifyYaml } from 'yaml';
import { emptyReviewMeta, isEmptyReviewMeta, type ReviewMeta, type ReviewMetaEntry } from './rfm-types';
```

Append this function:

```ts
/**
 * Serialize review metadata to deterministic YAML (sorted keys), or "" when
 * empty. Empty `comments`/`suggestions` maps are omitted. Deterministic output
 * is required so re-saving an unchanged document does not churn the file.
 */
export function serializeReviewMeta(meta: ReviewMeta): string {
  if (isEmptyReviewMeta(meta)) return '';
  const root: Record<string, unknown> = {};
  if (Object.keys(meta.comments).length > 0) root.comments = meta.comments;
  if (Object.keys(meta.suggestions).length > 0) root.suggestions = meta.suggestions;
  return stringifyYaml(root, { sortMapEntries: true });
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd ui && pnpm exec vitest run src/features/review/endmatter.test.ts`
Expected: PASS (all endmatter tests). If the `yaml` library quotes the bare `by`/`re` values differently than the fixture expects, adjust the fixture to match the library's canonical output — the contract that matters is **determinism and round-trip**, not exact quoting. (Re-run after adjusting.)

- [ ] **Step 5: Commit**

```bash
git add ui/src/features/review/endmatter.ts ui/src/features/review/endmatter.test.ts
git commit -m "feat(review): serialize review metadata to deterministic YAML

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Serialize Plate review marks → CriticMarkup (MdRules)

**Files:**
- Create: `ui/src/features/review/review-md-rules.ts`
- Test: `ui/src/features/review/review-md-rules.test.ts`

This task produces the Plate `MdRules` that emit CriticMarkup text for the three mark families. Comment/suggestion ids live on leaf keys (`comment_<id>`, `suggestion_<id>`); the rules read them and emit the inline marker plus a `{#id}` reference. Comment bodies/anchors: the rule emits `{==text==}{>>body<<}{#id}` where `body` is looked up from the bound `meta`.

- [ ] **Step 1: Write the failing test**

```ts
// ui/src/features/review/review-md-rules.test.ts
import { describe, expect, it } from 'vitest';
import { serializeReviewBody } from './review-md-rules';
import { emptyReviewMeta } from './rfm-types';

describe('serializeReviewBody', () => {
  it('emits a highlight mark as {==text==}', () => {
    const value = [{ type: 'p', children: [{ text: 'pick ' }, { text: 'this', highlight: true }, { text: '.' }] }];
    expect(serializeReviewBody(value, emptyReviewMeta())).toBe('pick {==this==}.');
  });

  it('emits an insert suggestion as {++text++}{#id}', () => {
    const value = [
      { type: 'p', children: [{ text: 'add ' }, { text: 'more', suggestion: true, suggestion_s1: { id: 's1', type: 'insert', userId: 'AI', createdAt: 0 } }] },
    ];
    expect(serializeReviewBody(value, emptyReviewMeta())).toBe('add {++more++}{#s1}');
  });

  it('emits a remove suggestion as {--text--}{#id}', () => {
    const value = [
      { type: 'p', children: [{ text: 'drop ', }, { text: 'this', suggestion: true, suggestion_s2: { id: 's2', type: 'remove', userId: 'user', createdAt: 0 } }] },
    ];
    expect(serializeReviewBody(value, emptyReviewMeta())).toBe('drop {--this--}{#s2}');
  });

  it('emits a comment as {==anchor==}{>>body<<}{#id} using the body from meta', () => {
    const value = [{ type: 'p', children: [{ text: 'see ' }, { text: 'here', comment: true, comment_c1: true }] }];
    const meta = { comments: { c1: { by: 'user', at: 'x', body: 'fix this' } }, suggestions: {} };
    expect(serializeReviewBody(value, meta)).toBe('see {==here==}{>>fix this<<}{#c1}');
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd ui && pnpm exec vitest run src/features/review/review-md-rules.test.ts`
Expected: FAIL — `Failed to resolve import './review-md-rules'`.

- [ ] **Step 3: Write minimal implementation**

```ts
// ui/src/features/review/review-md-rules.ts
import { createSlateEditor, type Descendant } from 'platejs';
import {
  BaseBlockquotePlugin, BaseBoldPlugin, BaseCodePlugin,
  BaseH1Plugin, BaseH2Plugin, BaseH3Plugin, BaseH4Plugin, BaseH5Plugin, BaseH6Plugin,
  BaseHorizontalRulePlugin, BaseItalicPlugin, BaseStrikethroughPlugin,
  BaseSubscriptPlugin, BaseSuperscriptPlugin, BaseUnderlinePlugin,
} from '@platejs/basic-nodes';
import { BaseHighlightPlugin } from '@platejs/basic-nodes';
import { BaseCodeBlockPlugin, BaseCodeLinePlugin, BaseCodeSyntaxPlugin } from '@platejs/code-block';
import { BaseLinkPlugin } from '@platejs/link';
import { BaseListPlugin } from '@platejs/list';
import { MarkdownPlugin } from '@platejs/markdown';
import { BaseParagraphPlugin } from 'platejs';
import remarkGfm from 'remark-gfm';

import { remarkInlineMarks } from '../editor/remark-inline-marks';
import type { ReviewMeta } from './rfm-types';

/** Read the suggestion data object off a leaf (key `suggestion_<id>`). */
function suggestionData(leaf: Record<string, unknown>): { id: string; type: 'insert' | 'remove' } | null {
  for (const key of Object.keys(leaf)) {
    if (key.startsWith('suggestion_') && typeof leaf[key] === 'object' && leaf[key] !== null) {
      const data = leaf[key] as { id?: string; type?: string };
      if (data.id && (data.type === 'insert' || data.type === 'remove')) {
        return { id: data.id, type: data.type };
      }
    }
  }
  return null;
}

/** Read the comment id off a leaf (key `comment_<id>`, excluding the draft key). */
function commentId(leaf: Record<string, unknown>): string | null {
  for (const key of Object.keys(leaf)) {
    if (key.startsWith('comment_') && key !== 'comment_draft' && leaf[key] === true) {
      return key.slice('comment_'.length);
    }
  }
  return null;
}

/** Build the Plate MdRules that serialize review marks to CriticMarkup. */
export function reviewMdRules(meta: ReviewMeta) {
  return {
    highlight: {
      mark: true,
      serialize: (leaf: { text: string }) => ({ type: 'text', value: `{==${leaf.text}==}` }),
    },
    suggestion: {
      mark: true,
      serialize: (leaf: Record<string, unknown> & { text: string }) => {
        const data = suggestionData(leaf);
        if (!data) return { type: 'text', value: leaf.text };
        const open = data.type === 'remove' ? '{--' : '{++';
        const close = data.type === 'remove' ? '--}' : '++}';
        return { type: 'text', value: `${open}${leaf.text}${close}{#${data.id}}` };
      },
    },
    comment: {
      mark: true,
      serialize: (leaf: Record<string, unknown> & { text: string }) => {
        const id = commentId(leaf);
        if (!id) return { type: 'text', value: leaf.text };
        const body = meta.comments[id]?.body ?? '';
        const bodyPart = body ? `{>>${body}<<}` : '';
        return { type: 'text', value: `{==${leaf.text}==}${bodyPart}{#${id}}` };
      },
    },
  };
}

function serializerEditor(meta: ReviewMeta) {
  return createSlateEditor({
    plugins: [
      BaseParagraphPlugin, BaseH1Plugin, BaseH2Plugin, BaseH3Plugin, BaseH4Plugin, BaseH5Plugin, BaseH6Plugin,
      BaseBlockquotePlugin, BaseHorizontalRulePlugin, BaseCodeBlockPlugin, BaseCodeLinePlugin, BaseCodeSyntaxPlugin,
      BaseBoldPlugin, BaseItalicPlugin, BaseCodePlugin, BaseStrikethroughPlugin, BaseUnderlinePlugin,
      BaseSubscriptPlugin, BaseSuperscriptPlugin, BaseHighlightPlugin, BaseListPlugin, BaseLinkPlugin,
      MarkdownPlugin.configure({ options: { remarkPlugins: [remarkGfm, remarkInlineMarks], rules: reviewMdRules(meta) } }),
    ],
  } as never) as ReturnType<typeof createSlateEditor> & {
    api: { markdown: { serialize: (options: { value: Descendant[] }) => string } };
  };
}

/** Serialize a Plate value's body to Markdown with review marks emitted as CriticMarkup. */
export function serializeReviewBody(value: Descendant[], meta: ReviewMeta): string {
  return serializerEditor(meta).api.markdown.serialize({ value }).replace(/\n+$/, '');
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd ui && pnpm exec vitest run src/features/review/review-md-rules.test.ts`
Expected: PASS (4 tests). This is a high-integration task: if `@platejs/markdown` wraps the marked run differently (e.g. extra spaces or escaping), adjust the rule's emitted `value` and/or the trailing-newline trim so the output matches the fixtures, keeping the CriticMarkup shape exact.

- [ ] **Step 5: Commit**

```bash
git add ui/src/features/review/review-md-rules.ts ui/src/features/review/review-md-rules.test.ts
git commit -m "feat(review): serialize Plate review marks to CriticMarkup

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Collapse adjacent delete+insert into a substitution

**Files:**
- Create: `ui/src/features/review/collapse-substitutions.ts`
- Test: `ui/src/features/review/collapse-substitutions.test.ts`

Plate models a replacement as a remove-leaf + insert-leaf sharing one id. After serialization they appear adjacently as `{--old--}{#id}{++new--}{#id}`. This pass collapses that pair to `{~~old~>new~~}{#id}`.

- [ ] **Step 1: Write the failing test**

```ts
// ui/src/features/review/collapse-substitutions.test.ts
import { describe, expect, it } from 'vitest';
import { collapseSubstitutions } from './collapse-substitutions';

describe('collapseSubstitutions', () => {
  it('collapses an adjacent remove+insert sharing an id', () => {
    expect(collapseSubstitutions('use {--rough--}{#s1}{++specific++}{#s1} wording')).toBe(
      'use {~~rough~>specific~~}{#s1} wording'
    );
  });

  it('also collapses insert-before-remove order', () => {
    expect(collapseSubstitutions('use {++specific++}{#s1}{--rough--}{#s1} wording')).toBe(
      'use {~~rough~>specific~~}{#s1} wording'
    );
  });

  it('leaves standalone insert/delete untouched', () => {
    expect(collapseSubstitutions('add {++x++}{#s1} and drop {--y--}{#s2}')).toBe(
      'add {++x++}{#s1} and drop {--y--}{#s2}'
    );
  });

  it('does not merge a remove+insert with different ids', () => {
    const input = '{--old--}{#s1}{++new++}{#s2}';
    expect(collapseSubstitutions(input)).toBe(input);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd ui && pnpm exec vitest run src/features/review/collapse-substitutions.test.ts`
Expected: FAIL — `Failed to resolve import './collapse-substitutions'`.

- [ ] **Step 3: Write minimal implementation**

```ts
// ui/src/features/review/collapse-substitutions.ts

// Matches "{--old--}{#id}{++new++}{#id}" or the insert-first variant, requiring
// the same id on both halves (backreference \2 / \5). Inner text excludes the
// relevant close delimiter so spans stay tight.
const DEL_THEN_INS = /\{--((?:(?!--\}).)*)--\}\{#([A-Za-z0-9_-]+)\}\{\+\+((?:(?!\+\+\}).)*)\+\+\}\{#\2\}/g;
const INS_THEN_DEL = /\{\+\+((?:(?!\+\+\}).)*)\+\+\}\{#([A-Za-z0-9_-]+)\}\{--((?:(?!--\}).)*)--\}\{#\2\}/g;

/** Collapse adjacent delete+insert pairs sharing an id into `{~~old~>new~~}{#id}`. */
export function collapseSubstitutions(markdown: string): string {
  return markdown
    .replace(DEL_THEN_INS, (_m, oldText, id, newText) => `{~~${oldText}~>${newText}~~}{#${id}}`)
    .replace(INS_THEN_DEL, (_m, newText, id, oldText) => `{~~${oldText}~>${newText}~~}{#${id}}`);
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd ui && pnpm exec vitest run src/features/review/collapse-substitutions.test.ts`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add ui/src/features/review/collapse-substitutions.ts ui/src/features/review/collapse-substitutions.test.ts
git commit -m "feat(review): collapse adjacent del+ins into a substitution

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Parse CriticMarkup in a Plate value into marks

**Files:**
- Create: `ui/src/features/review/apply-critic-markup.ts`
- Test: `ui/src/features/review/apply-critic-markup.test.ts`

After `markdown.deserialize`, CriticMarkup is still literal text inside text leaves. `applyCriticMarkup` walks the value, finds complete CriticMarkup tokens **within a single text leaf**, and rewrites them into marked leaves. It resolves metadata from `meta`, synthesizes missing ids with `nanoid`, and extracts inline comment bodies into `meta`. Leaves inside code blocks/marks are skipped (CriticMarkup stays literal). Tokens wrapping inline-formatted content (split across leaves) are a documented v1 limitation.

- [ ] **Step 1: Write the failing test**

```ts
// ui/src/features/review/apply-critic-markup.test.ts
import { describe, expect, it } from 'vitest';
import { applyCriticMarkup } from './apply-critic-markup';
import { emptyReviewMeta } from './rfm-types';

const p = (children: unknown[]) => ({ type: 'p', children });

describe('applyCriticMarkup', () => {
  it('parses an insert suggestion and reads metadata from meta', () => {
    const meta = { comments: {}, suggestions: { s1: { by: 'AI', at: '2026-01-01T00:00:00.000Z' } } };
    const { value } = applyCriticMarkup([p([{ text: 'add {++more++}{#s1}' }])], meta);
    expect(value).toEqual([p([{ text: 'add ' }, { text: 'more', suggestion: true, suggestion_s1: { id: 's1', type: 'insert', userId: 'AI', createdAt: expect.any(Number) } }])]);
  });

  it('parses a remove suggestion', () => {
    const { value } = applyCriticMarkup([p([{ text: 'drop {--this--}{#s2}' }])], { comments: {}, suggestions: { s2: { by: 'user', at: 'x' } } });
    const leaf = (value[0] as { children: Record<string, unknown>[] }).children[1];
    expect(leaf.text).toBe('this');
    expect(leaf.suggestion).toBe(true);
    expect((leaf.suggestion_s2 as { type: string }).type).toBe('remove');
  });

  it('splits a substitution into a remove leaf and an insert leaf sharing the id', () => {
    const { value } = applyCriticMarkup([p([{ text: 'use {~~rough~>specific~~}{#s3}' }])], { comments: {}, suggestions: { s3: { by: 'AI', at: 'x' } } });
    const children = (value[0] as { children: Record<string, unknown>[] }).children;
    const remove = children.find((c) => (c.suggestion_s3 as { type?: string })?.type === 'remove');
    const insert = children.find((c) => (c.suggestion_s3 as { type?: string })?.type === 'insert');
    expect(remove?.text).toBe('rough');
    expect(insert?.text).toBe('specific');
  });

  it('parses a comment anchor and lifts the inline body into meta', () => {
    const meta = { comments: { c1: { by: 'user', at: '2026-01-01T00:00:00.000Z' } }, suggestions: {} };
    const { value, meta: outMeta } = applyCriticMarkup([p([{ text: 'see {==here==}{>>fix this<<}{#c1}' }])], meta);
    const leaf = (value[0] as { children: Record<string, unknown>[] }).children[1];
    expect(leaf.text).toBe('here');
    expect(leaf.comment).toBe(true);
    expect(leaf.comment_c1).toBe(true);
    expect(outMeta.comments.c1.body).toBe('fix this');
  });

  it('parses a bare highlight as a highlight mark (no comment)', () => {
    const { value } = applyCriticMarkup([p([{ text: 'pick {==this==} please' }])], emptyReviewMeta());
    expect(value).toEqual([p([{ text: 'pick ' }, { text: 'this', highlight: true }, { text: ' please' }])]);
  });

  it('synthesizes an id when a marker has none', () => {
    const { value, meta } = applyCriticMarkup([p([{ text: 'add {++x++}' }])], emptyReviewMeta());
    const leaf = (value[0] as { children: Record<string, unknown>[] }).children[1];
    const data = leaf.suggestion_ ? null : Object.entries(leaf).find(([k]) => k.startsWith('suggestion_'));
    expect(data).toBeTruthy();
    expect(Object.keys(meta.suggestions)).toHaveLength(1);
  });

  it('leaves CriticMarkup inside a code leaf literal', () => {
    const { value } = applyCriticMarkup([p([{ text: '{++x++}', code: true }])], emptyReviewMeta());
    expect(value).toEqual([p([{ text: '{++x++}', code: true }])]);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd ui && pnpm exec vitest run src/features/review/apply-critic-markup.test.ts`
Expected: FAIL — `Failed to resolve import './apply-critic-markup'`.

- [ ] **Step 3: Write minimal implementation**

```ts
// ui/src/features/review/apply-critic-markup.ts
import { nanoid } from 'nanoid';
import type { ReviewMeta } from './rfm-types';

type Leaf = Record<string, unknown> & { text: string };
type Node = Record<string, unknown> & { children?: Node[]; text?: string };

const CODE_BLOCK_TYPES = new Set(['code_block', 'code_line']);

// One regex with named groups for each marker family, plus an optional {#id}.
const TOKEN = new RegExp(
  [
    String.raw`\{==(?<hl>(?:(?!==\}).)*)==\}(?:\{>>(?<cbody>(?:(?!<<\}).)*)<<\})?(?:\{#(?<cid>[A-Za-z0-9_-]+)\})?`,
    String.raw`\{~~(?<sold>(?:(?!~>).)*)~>(?<snew>(?:(?!~~\}).)*)~~\}(?:\{#(?<subid>[A-Za-z0-9_-]+)\})?`,
    String.raw`\{\+\+(?<ins>(?:(?!\+\+\}).)*)\+\+\}(?:\{#(?<insid>[A-Za-z0-9_-]+)\})?`,
    String.raw`\{--(?<del>(?:(?!--\}).)*)--\}(?:\{#(?<delid>[A-Za-z0-9_-]+)\})?`,
    String.raw`\{>>(?<conly>(?:(?!<<\}).)*)<<\}(?:\{#(?<conlyid>[A-Za-z0-9_-]+)\})?`,
  ].join('|'),
  'g'
);

function ensureSuggestion(meta: ReviewMeta, id: string): { by: string; at: string } {
  const entry = meta.suggestions[id] ?? { by: 'unknown', at: new Date().toISOString() };
  meta.suggestions[id] = entry;
  return entry;
}

function ensureComment(meta: ReviewMeta, id: string, body?: string): void {
  const entry = meta.comments[id] ?? { by: 'unknown', at: new Date().toISOString() };
  if (body && !entry.body) entry.body = body;
  meta.comments[id] = entry;
}

/** Split one text leaf's `text` into plain + marked leaves. Other leaf props (marks like bold) are carried onto each segment. */
function expandLeaf(leaf: Leaf, meta: ReviewMeta): Leaf[] {
  const { text, ...rest } = leaf;
  if (rest.code) return [leaf]; // code is literal
  const out: Leaf[] = [];
  let last = 0;
  for (const match of text.matchAll(TOKEN)) {
    const start = match.index ?? 0;
    if (start > last) out.push({ ...rest, text: text.slice(last, start) } as Leaf);
    const g = match.groups ?? {};
    if (g.hl !== undefined) {
      if (g.cbody !== undefined || g.cid !== undefined) {
        const id = g.cid ?? nanoid();
        ensureComment(meta, id, g.cbody);
        out.push({ ...rest, text: g.hl, comment: true, [`comment_${id}`]: true } as Leaf);
      } else {
        out.push({ ...rest, text: g.hl, highlight: true } as Leaf);
      }
    } else if (g.sold !== undefined) {
      const id = g.subid ?? nanoid();
      ensureSuggestion(meta, id);
      const by = meta.suggestions[id].by;
      out.push({ ...rest, text: g.sold, suggestion: true, [`suggestion_${id}`]: { id, type: 'remove', userId: by, createdAt: 0 } } as Leaf);
      out.push({ ...rest, text: g.snew, suggestion: true, [`suggestion_${id}`]: { id, type: 'insert', userId: by, createdAt: 0 } } as Leaf);
    } else if (g.ins !== undefined) {
      const id = g.insid ?? nanoid();
      ensureSuggestion(meta, id);
      out.push({ ...rest, text: g.ins, suggestion: true, [`suggestion_${id}`]: { id, type: 'insert', userId: meta.suggestions[id].by, createdAt: 0 } } as Leaf);
    } else if (g.del !== undefined) {
      const id = g.delid ?? nanoid();
      ensureSuggestion(meta, id);
      out.push({ ...rest, text: g.del, suggestion: true, [`suggestion_${id}`]: { id, type: 'remove', userId: meta.suggestions[id].by, createdAt: 0 } } as Leaf);
    } else if (g.conly !== undefined) {
      const id = g.conlyid ?? nanoid();
      ensureComment(meta, id, g.conly);
      // A standalone comment with no highlight anchors to a zero-width point;
      // attach the mark to a single space so the rail has something to target.
      out.push({ ...rest, text: ' ', comment: true, [`comment_${id}`]: true } as Leaf);
    }
    last = start + match[0].length;
  }
  if (last < text.length) out.push({ ...rest, text: text.slice(last) } as Leaf);
  return out.length > 0 ? out : [leaf];
}

function walk(node: Node, meta: ReviewMeta, inCode: boolean): Node {
  if (Array.isArray(node.children)) {
    const childInCode = inCode || (typeof node.type === 'string' && CODE_BLOCK_TYPES.has(node.type));
    const children: Node[] = [];
    for (const child of node.children) {
      if (typeof child.text === 'string' && !Array.isArray(child.children)) {
        if (childInCode) children.push(child);
        else children.push(...(expandLeaf(child as Leaf, meta) as Node[]));
      } else {
        children.push(walk(child, meta, childInCode));
      }
    }
    return { ...node, children };
  }
  return node;
}

/**
 * Rewrite CriticMarkup tokens found inside text leaves into Plate review marks.
 * Returns a new value and a (possibly augmented) meta with synthesized ids and
 * lifted inline comment bodies.
 */
export function applyCriticMarkup(value: Node[], meta: ReviewMeta): { value: Node[]; meta: ReviewMeta } {
  const nextMeta: ReviewMeta = { comments: { ...meta.comments }, suggestions: { ...meta.suggestions } };
  const root = walk({ type: 'root', children: value }, nextMeta, false);
  return { value: root.children ?? [], meta: nextMeta };
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd ui && pnpm exec vitest run src/features/review/apply-critic-markup.test.ts`
Expected: PASS (7 tests). This is the highest-risk task. If a fixture's leaf-shape expectation is off (e.g. property ordering, the `createdAt` placeholder), align the implementation to the **contract the test asserts** (mark keys + text + type), not the other way around. Adjust `createdAt` handling if downstream tasks need real timestamps.

- [ ] **Step 5: Commit**

```bash
git add ui/src/features/review/apply-critic-markup.ts ui/src/features/review/apply-critic-markup.test.ts
git commit -m "feat(review): parse CriticMarkup in Plate values into review marks

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: The codec orchestrator + round-trip & idempotence

**Files:**
- Modify: `ui/src/features/editor/markdown-codec.ts` (export a reusable base-plugins list)
- Create: `ui/src/features/review/rfm-codec.ts`
- Test: `ui/src/features/review/rfm-codec.test.ts`

- [ ] **Step 1: Export the base plugin list from `markdown-codec.ts`**

In `ui/src/features/editor/markdown-codec.ts`, extract the plugin array used by `editor()` into an exported constant so the codec reuses the exact same base surface (single source of truth for which nodes/marks round-trip):

```ts
// Add near the top of markdown-codec.ts, replacing the inline array inside editor():
export const baseMarkdownPlugins = [
  BaseParagraphPlugin, BaseH1Plugin, BaseH2Plugin, BaseH3Plugin, BaseH4Plugin, BaseH5Plugin, BaseH6Plugin,
  BaseBlockquotePlugin, BaseHorizontalRulePlugin, BaseCodeBlockPlugin, BaseCodeLinePlugin, BaseCodeSyntaxPlugin,
  BaseBoldPlugin, BaseItalicPlugin, BaseCodePlugin, BaseStrikethroughPlugin, BaseUnderlinePlugin,
  BaseSubscriptPlugin, BaseSuperscriptPlugin, BaseListPlugin, BaseLinkPlugin,
];
```

Then change `editor()` to build from it:

```ts
function editor() {
  return createSlateEditor({
    plugins: [
      ...baseMarkdownPlugins,
      MarkdownPlugin.configure({ options: { remarkPlugins: [remarkGfm, remarkInlineMarks] } }),
    ],
  } as never) as /* keep the existing return type annotation */;
}
```

(Leave `markdownToPlateValue`/`plateValueToMarkdown` unchanged — Plan 2 decides whether the live editor swaps to the review codec.)

- [ ] **Step 2: Write the failing test**

```ts
// ui/src/features/review/rfm-codec.test.ts
import { describe, expect, it } from 'vitest';
import { markdownToReview, reviewToMarkdown } from './rfm-codec';

describe('rfm-codec round-trip', () => {
  it('round-trips a comment with endmatter', () => {
    const md = 'See {==here==}{>>fix this<<}{#c1}.\n\n---\ncomments:\n  c1:\n    at: "2026-01-01T00:00:00.000Z"\n    by: user\n';
    const { value, meta } = markdownToReview(md);
    expect(meta.comments.c1.by).toBe('user');
    const out = reviewToMarkdown(value, meta);
    expect(out).toContain('{==here==}{>>fix this<<}{#c1}');
    expect(out).toContain('comments:');
  });

  it('round-trips a substitution suggestion', () => {
    const md = 'Use {~~rough~>specific~~}{#s1} wording.\n\n---\nsuggestions:\n  s1:\n    at: "2026-01-01T00:00:00.000Z"\n    by: AI\n';
    const { value, meta } = markdownToReview(md);
    const out = reviewToMarkdown(value, meta);
    expect(out).toContain('{~~rough~>specific~~}{#s1}');
  });

  it('is idempotent: parse→serialize→parse→serialize is stable', () => {
    const md = 'Use {~~rough~>specific~~}{#s1} wording.\n\n---\nsuggestions:\n  s1:\n    at: "2026-01-01T00:00:00.000Z"\n    by: AI\n';
    const first = reviewToMarkdown(...Object.values(markdownToReview(md)) as [never, never]);
    const second = reviewToMarkdown(...Object.values(markdownToReview(first)) as [never, never]);
    expect(second).toBe(first);
  });

  it('only emits endmatter entries for ids still present as marks (orphan prune)', () => {
    const md = 'Plain text, no markers.\n\n---\ncomments:\n  c1:\n    at: "2026-01-01T00:00:00.000Z"\n    by: user\n';
    const { value, meta } = markdownToReview(md);
    const out = reviewToMarkdown(value, meta);
    expect(out).not.toContain('comments:');
    expect(out.trim()).toBe('Plain text, no markers.');
  });
});
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cd ui && pnpm exec vitest run src/features/review/rfm-codec.test.ts`
Expected: FAIL — `Failed to resolve import './rfm-codec'`.

- [ ] **Step 4: Write minimal implementation**

```ts
// ui/src/features/review/rfm-codec.ts
import { createSlateEditor, type Descendant } from 'platejs';
import { MarkdownPlugin } from '@platejs/markdown';
import remarkGfm from 'remark-gfm';

import { baseMarkdownPlugins } from '../editor/markdown-codec';
import { remarkInlineMarks } from '../editor/remark-inline-marks';
import { applyCriticMarkup } from './apply-critic-markup';
import { collapseSubstitutions } from './collapse-substitutions';
import { serializeReviewBody } from './review-md-rules';
import { serializeReviewMeta } from './endmatter';
import { splitEndmatter } from './endmatter';
import { emptyReviewMeta, type ReviewMeta } from './rfm-types';

interface ReviewDocument {
  value: Descendant[];
  meta: ReviewMeta;
}

function deserializeEditor() {
  return createSlateEditor({
    plugins: [...baseMarkdownPlugins, MarkdownPlugin.configure({ options: { remarkPlugins: [remarkGfm, remarkInlineMarks] } })],
  } as never) as ReturnType<typeof createSlateEditor> & {
    api: { markdown: { deserialize: (md: string) => Descendant[] } };
  };
}

/** Markdown (RFM) → Plate value with review marks + parsed metadata. */
export function markdownToReview(markdown: string): ReviewDocument {
  const { body, meta } = splitEndmatter(markdown);
  const rawValue = deserializeEditor().api.markdown.deserialize(body);
  const { value, meta: mergedMeta } = applyCriticMarkup(rawValue as never, meta ?? emptyReviewMeta());
  return { value: value as Descendant[], meta: mergedMeta };
}

/** Collect the comment/suggestion ids still present as marks in the value. */
function liveIds(value: Descendant[]): { comments: Set<string>; suggestions: Set<string> } {
  const comments = new Set<string>();
  const suggestions = new Set<string>();
  const visit = (node: Record<string, unknown>) => {
    for (const key of Object.keys(node)) {
      if (key.startsWith('comment_') && key !== 'comment_draft' && node[key] === true) comments.add(key.slice('comment_'.length));
      if (key.startsWith('suggestion_') && typeof node[key] === 'object' && node[key] !== null) {
        const id = (node[key] as { id?: string }).id;
        if (id) suggestions.add(id);
      }
    }
    if (Array.isArray((node as { children?: unknown[] }).children)) {
      for (const child of (node as { children: Record<string, unknown>[] }).children) visit(child);
    }
  };
  for (const node of value as Record<string, unknown>[]) visit(node);
  return { comments, suggestions };
}

/** Drop metadata entries whose anchor mark no longer exists (keep replies whose parent is live). */
function pruneMeta(meta: ReviewMeta, live: { comments: Set<string>; suggestions: Set<string> }): ReviewMeta {
  const pruned = emptyReviewMeta();
  for (const [id, entry] of Object.entries(meta.comments)) {
    if (live.comments.has(id) || (entry.re && live.comments.has(entry.re))) pruned.comments[id] = entry;
  }
  for (const [id, entry] of Object.entries(meta.suggestions)) {
    if (live.suggestions.has(id)) pruned.suggestions[id] = entry;
  }
  return pruned;
}

/** Plate value + metadata → Markdown (RFM). */
export function reviewToMarkdown(value: Descendant[], meta: ReviewMeta): string {
  const live = liveIds(value);
  const pruned = pruneMeta(meta, live);
  const body = collapseSubstitutions(serializeReviewBody(value, pruned));
  const endmatter = serializeReviewMeta(pruned);
  return endmatter ? `${body}\n\n---\n${endmatter}` : `${body}\n`;
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cd ui && pnpm exec vitest run src/features/review/rfm-codec.test.ts`
Expected: PASS (4 tests). If idempotence fails, the usual causes are (a) non-deterministic YAML key order — confirm `sortMapEntries`, and (b) trailing-whitespace differences between the endmatter and no-endmatter branches — normalize the body's trailing newline in `reviewToMarkdown`. Fix and re-run.

- [ ] **Step 6: Run the full review suite + typecheck**

Run: `cd ui && pnpm exec vitest run src/features/review && pnpm typecheck`
Expected: all review tests PASS; `tsc -b` reports no errors.

- [ ] **Step 7: Commit**

```bash
git add ui/src/features/editor/markdown-codec.ts ui/src/features/review/rfm-codec.ts ui/src/features/review/rfm-codec.test.ts
git commit -m "feat(review): add RFM codec orchestrator with round-trip + orphan prune

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: Conformance fixtures + edge cases

**Files:**
- Create: `ui/src/features/review/__fixtures__/` (RFM sample docs)
- Test: `ui/src/features/review/rfm-conformance.test.ts`

Borrow representative fixtures modeled on Roughdraft's published conformance set (we adopted its spec). Each fixture is one reported case (no in-body loops).

- [ ] **Step 1: Add fixtures**

Create these files verbatim:

`ui/src/features/review/__fixtures__/threaded-comment.md`:
```markdown
Please revisit {==this sentence==}{>>Needs a source<<}{#c1}.

---
comments:
  c1:
    at: "2026-04-28T12:00:00.000Z"
    by: user
  c2:
    at: "2026-04-28T12:05:00.000Z"
    body: I can add one from the intro.
    by: AI
    re: c1
```

`ui/src/features/review/__fixtures__/code-literal.md`:
```markdown
Inline code stays literal: `{==not a comment==}`.

```text
{++not a suggestion++}
```
```

- [ ] **Step 2: Write the failing test**

```ts
// ui/src/features/review/rfm-conformance.test.ts
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { markdownToReview, reviewToMarkdown } from './rfm-codec';

const fixture = (name: string) =>
  readFileSync(fileURLToPath(new URL(`./__fixtures__/${name}`, import.meta.url)), 'utf-8');

describe('RFM conformance', () => {
  it('keeps a threaded reply (re:) in metadata and round-trips its parent anchor', () => {
    const { value, meta } = markdownToReview(fixture('threaded-comment.md'));
    expect(meta.comments.c2.re).toBe('c1');
    const out = reviewToMarkdown(value, meta);
    expect(out).toContain('{==this sentence==}');
    expect(out).toContain('re: c1');
  });

  it('treats CriticMarkup inside inline code and fenced blocks as literal text', () => {
    const { value } = markdownToReview(fixture('code-literal.md'));
    const out = reviewToMarkdown(value, { comments: {}, suggestions: {} });
    expect(out).toContain('`{==not a comment==}`');
    expect(out).toContain('{++not a suggestion++}');
  });

  it('treats an unclosed marker as literal text (no crash)', () => {
    const { value, meta } = markdownToReview('A stray {++ open marker.\n');
    expect(meta.suggestions).toEqual({});
    expect(reviewToMarkdown(value, meta)).toContain('{++ open marker.');
  });
});
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cd ui && pnpm exec vitest run src/features/review/rfm-conformance.test.ts`
Expected: FAIL — module/fixtures not found, or assertions fail.

- [ ] **Step 4: Make it pass**

These exercise existing code paths. Expected real fixes if they fail:
- **Code-literal**: confirm `applyCriticMarkup`'s `inCode` propagation covers fenced blocks (`code_block`/`code_line` types) and the `code` leaf mark. If `@platejs/markdown` represents inline code with a different leaf key, add it to the skip check in `apply-critic-markup.ts`.
- **Unclosed marker**: the `TOKEN` regex requires a closing delimiter, so an unclosed `{++` simply never matches and stays literal — verify, and add the leaf to the skip path only if needed.

Re-run: `cd ui && pnpm exec vitest run src/features/review/rfm-conformance.test.ts`
Expected: PASS (3 tests).

- [ ] **Step 5: Full suite + typecheck**

Run: `cd ui && pnpm exec vitest run src/features/review && pnpm typecheck`
Expected: all PASS; no type errors.

- [ ] **Step 6: Commit**

```bash
git add ui/src/features/review/__fixtures__ ui/src/features/review/rfm-conformance.test.ts ui/src/features/review/apply-critic-markup.ts
git commit -m "test(review): RFM conformance fixtures (threads, code-literal, unclosed)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Done criteria (Plan 1)

- `markdownToReview` / `reviewToMarkdown` round-trip all five markers + threaded replies through RFM Markdown.
- Round-trip is **idempotent** for review regions (deterministic endmatter).
- Orphaned metadata is pruned on serialize; false trailing `---` blocks are not hijacked; CriticMarkup in code is literal; unclosed markers degrade to text.
- All review tests + `pnpm typecheck` pass.

## Known v1 limitations (carried by the codec)

- **Inline comment bodies and the `<<}` delimiter:** Task 4 always emits root comment bodies inline as `{>>body<<}`. A body containing the literal `<<}` close delimiter would corrupt the marker. The spec's fallback (route such bodies to an endmatter `body:` instead) is **deferred** — track as a follow-up in Plan 2's store work, where comment bodies are authored and the escape/route decision is cheap to apply at write time. Until then, the composer should reject `<<}` in comment text.
- **Formatted content inside a CriticMarkup span** (e.g. `{++**bold**++}`) is split across leaves by `remark` and not re-marked by the single-leaf `applyCriticMarkup` scan. v1 supports plain-text spans (the common case); cross-leaf spans are a documented limitation.
- **Partial (non-coincident) overlapping comments** are approximated by adjacent anchors, per the design doc.

## Follow-on plans (to be authored against this concrete API)

- **Plan 2 — editor + store wiring:** add `@platejs/comment` + `@platejs/suggestion` to the live editor (`review-kit.ts`), a Zustand `discussion-store.ts` hydrated from `markdownToReview` and serialized via `reviewToMarkdown`, nanoid id minting, live Suggesting mode, accept/reject, and the "value *or* store triggers save" bridge in `PlateMarkdownEditor.tsx`.
- **Plan 3 — review UI:** adapt Potion's `floating-discussion-app`, `block-suggestion-app`, `comment-app`, toolbar buttons, and Suggesting pill to quarry tokens + the Zustand store; editor-integration + Playwright e2e.
- **Verification (cross-cutting):** confirm `quarry-git` treats a trailing `---\n<yaml>` endmatter as opaque content (design doc, Section 4).
