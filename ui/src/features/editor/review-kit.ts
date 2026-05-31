import { CommentPlugin } from '@platejs/comment/react';
import { SuggestionPlugin } from '@platejs/suggestion/react';
import { CommentLeaf, SuggestionLeaf } from './review-leaves';

// Review-layer marks for the live editor. Rendering/UI (leaf styling, rail,
// toolbar) is a later plan; this only registers the marks + the comment
// shortcut + enables suggesting mode (toggled elsewhere). currentUserId is set
// on the editor at mount (see PlateMarkdownEditor) BEFORE suggesting is enabled.
export const reviewKit = [
  CommentPlugin.configure({
    shortcuts: { setDraft: { keys: 'mod+shift+m' } },
    render: { node: CommentLeaf },
  }),
  SuggestionPlugin.configure({
    render: { node: SuggestionLeaf },
  }),
];
