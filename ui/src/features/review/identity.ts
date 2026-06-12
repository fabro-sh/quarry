// The free-form author label stamped on review items created in this editor.
// Quarry has no user accounts; humans self-declare this value and agents write
// their own `by:` label directly into the Markdown.
export const DEFAULT_AUTHOR = 'user';
const AUTHOR_STORAGE_KEY = 'quarry:author';

export function normalizeAuthor(value: string | null | undefined): string {
  const trimmed = value?.trim() ?? '';
  return trimmed || DEFAULT_AUTHOR;
}

export function loadAuthor(storage?: Storage): string {
  const target = storage ?? (typeof window === 'undefined' ? undefined : window.localStorage);
  return normalizeAuthor(target?.getItem(AUTHOR_STORAGE_KEY));
}

export function saveAuthor(value: string, storage?: Storage): string {
  const author = normalizeAuthor(value);
  const target = storage ?? (typeof window === 'undefined' ? undefined : window.localStorage);
  if (target) {
    if (author === DEFAULT_AUTHOR) target.removeItem(AUTHOR_STORAGE_KEY);
    else target.setItem(AUTHOR_STORAGE_KEY, author);
  }
  return author;
}

// True when the user explicitly chose a name (the raw key holds a non-blank value).
// `loadAuthor()` cannot distinguish "never asked" from "chose the default".
export function hasStoredAuthor(storage?: Storage): boolean {
  const target = storage ?? (typeof window === 'undefined' ? undefined : window.localStorage);
  return Boolean(target?.getItem(AUTHOR_STORAGE_KEY)?.trim());
}

export function currentAuthor(): string {
  if (typeof window === 'undefined') return DEFAULT_AUTHOR;
  return loadAuthor();
}
