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
