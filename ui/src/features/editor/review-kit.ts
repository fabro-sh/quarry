import { CommentPlugin } from '@platejs/comment/react';
import { SuggestionPlugin } from '@platejs/suggestion/react';
import { CommentDraftPlugin } from '../review/comment-draft';
import { CommentLeaf, SuggestionLeaf } from './review-leaves';

// Review-layer marks for the live editor. Rendering/UI (leaf styling, rail,
// toolbar) is a later plan; this only registers the marks + the comment
// shortcut + enables suggesting mode (toggled elsewhere). currentUserId is set
// on the editor at mount (see PlateMarkdownEditor) BEFORE suggesting is enabled.
//
// The comment shortcut lives on CommentDraftPlugin, NOT on CommentPlugin:
// Plate's own setDraft transform marks the document, and document state is
// shared live over Yjs — drafts must stay client-local until submitted.
export const reviewKit = [
  CommentPlugin.configure({
    render: { node: CommentLeaf },
  }),
  CommentDraftPlugin,
  SuggestionPlugin.configure({
    render: { node: SuggestionLeaf },
  }),
];
