// The free-form author label stamped on review items created in this editor.
// Quarry has no user accounts; humans are "user" and agents write their own
// `by:` label directly into the Markdown. Centralized so Plan 3 / future config
// can override it.
const DEFAULT_AUTHOR = 'user';

export function currentAuthor(): string {
  return DEFAULT_AUTHOR;
}
