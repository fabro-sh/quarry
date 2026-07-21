mod block_capabilities;
mod markdown;
mod markdown_writer;
mod normalize;
mod reconcile;
mod review;
mod rows;
mod session_doc;
mod slate;
mod text_diff;
mod trailing;
mod yjs_builder;

pub use block_capabilities::{
    BlockCapabilities, BlockContentModel, InlineSyntax, block_capabilities,
    can_promote_full_text_delete, carries_inline_content, is_known_block_type, known_block_types,
    uses_literal_inline_syntax,
};
pub use markdown::{block_markdown_to_slate, block_markdown_to_slate_raw, split_markdown_blocks};
pub use markdown_writer::{is_known_inline_mark, slate_to_markdown};
pub use reconcile::{ReconcileBase, ReconcileConflict, ReconcileOp, ReconcileOutcome, reconcile};
pub use review::{
    ReviewCommentMarker, ReviewDocument, ReviewMarkers, ReviewMeta, ReviewMetaEntry,
    ReviewMetaPatch, ReviewSuggestionKind, ReviewSuggestionMarker, has_review_endmatter,
    hydrate_inline_comment_bodies, inline_comment_body, parse_review_document,
    review_block_to_slate, review_blocks_to_slate, review_markdown_to_slate, review_markers,
    review_meta_with_inline_comment_bodies, split_review_endmatter,
};
pub use rows::{
    BlockRow, LinkRange, MarkRun, block_rows_to_markdown, block_rows_to_nodes, is_utf16_boundary,
    markdown_to_block_rows, utf16_len,
};
pub use session_doc::{
    SessionAnchor, SessionAnchorKind, SessionProjection, project_session_nodes,
    read_review_meta_from_map, reconcile_session_children, seed_session_nodes,
};
pub use slate::{Attrs, Node, attrs};
pub use text_diff::{
    MULTI_HUNK_CHAR_LIMIT, TextDiff, utf16_text_diff, utf16_text_diff_hunks,
    utf16_text_diff_hunks_bounded,
};
pub use trailing::{is_empty_paragraph, strip_trailing_empty_paragraphs};
pub use yjs_builder::{
    BuiltNode, apply_built, apply_review_patch_to_map, build_nodes, encode_update_v1_from_built,
    encode_update_v1_from_built_with_review, write_review_meta_to_map, xmltext_to_slate,
};

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("unsupported collab codec construct: {0}")]
pub struct Unsupported(pub String);

impl Unsupported {
    pub fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }

    pub fn context(self, context: impl AsRef<str>) -> Self {
        Self(format!("{}: {}", context.as_ref(), self.0))
    }
}
