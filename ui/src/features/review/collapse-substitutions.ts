import { nanoid } from 'nanoid';

// Matches "{--old--}{#id}{++new++}{#id}" or the insert-first variant, requiring
// the same id on both halves (backreference \2 / \5). Inner text excludes the
// relevant close delimiter so spans stay tight.
const DEL_THEN_INS = /\{--((?:(?!--\}).)*)--\}\{#([A-Za-z0-9_-]+)\}\{\+\+((?:(?!\+\+\}).)*)\+\+\}\{#\2\}/g;
const INS_THEN_DEL = /\{\+\+((?:(?!\+\+\}).)*)\+\+\}\{#([A-Za-z0-9_-]+)\}\{--((?:(?!--\}).)*)--\}\{#\2\}/g;

// Matches "{~~old~>new~~}" with an optional "{#id}". Inner spans exclude their
// delimiters so they stay tight.
const SUBSTITUTION = /\{~~((?:(?!~>).)*)~>((?:(?!~~\}).)*)~~\}(?:\{#([A-Za-z0-9_-]+)\})?/g;

/** Collapse adjacent delete+insert pairs sharing an id into `{~~old~>new~~}{#id}`. */
export function collapseSubstitutions(markdown: string): string {
  return markdown
    .replace(DEL_THEN_INS, (_m, oldText, id, newText) => `{~~${oldText}~>${newText}~~}{#${id}}`)
    .replace(INS_THEN_DEL, (_m, newText, id, oldText) => `{~~${oldText}~>${newText}~~}{#${id}}`);
}

/**
 * Expand `{~~old~>new~~}{#id}` into the id-paired delete+insert form
 * `{--old--}{#id}{++new++}{#id}`. This is the inverse of `collapseSubstitutions`
 * and runs before Markdown parsing so the substitution's `~~` is never seen as
 * GFM strikethrough. A shared id is synthesized when the token has none, so both
 * halves stay paired.
 */
export function expandSubstitutions(markdown: string): string {
  return markdown.replace(SUBSTITUTION, (_m, oldText, newText, id) => {
    const sharedId = id ?? nanoid();
    return `{--${oldText}--}{#${sharedId}}{++${newText}++}{#${sharedId}}`;
  });
}
