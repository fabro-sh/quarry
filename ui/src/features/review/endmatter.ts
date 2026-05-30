import { parse as parseYaml, stringify as stringifyYaml } from 'yaml';
import { emptyReviewMeta, isEmptyReviewMeta, type ReviewMeta, type ReviewMetaEntry } from './rfm-types';

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
  for (const [id, raw] of Object.entries(value)) {
    if (typeof raw !== 'object' || raw === null) continue;
    const entry: Record<string, unknown> = { ...raw };
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
