mod ast;
mod document_export;
mod document_import;
mod markdown;
mod markdown_writer;
mod normalize;
mod reconcile;
mod review;
mod rows;
mod text_diff;
mod trailing;

pub use ast::{Attrs, Node, attrs};
pub use document_export::{block_views_to_rows, document_to_block_rows, document_to_markdown};
pub use document_import::{
    import_markdown_document, import_markdown_with_existing_blocks, import_review_nodes,
};
pub use markdown::{block_markdown_to_nodes, block_markdown_to_nodes_raw, split_markdown_blocks};
pub use markdown_writer::{is_known_inline_mark, nodes_to_markdown};
pub use quarry_document::{
    BlockCapabilities, BlockContentModel, InlineSyntax, block_capabilities,
    can_promote_full_text_delete, carries_inline_content, is_known_block_type, known_block_types,
    uses_literal_inline_syntax,
};
pub use reconcile::{ReconcileBase, ReconcileConflict, ReconcileOp, ReconcileOutcome, reconcile};
pub use review::{
    ReviewCommentMarker, ReviewDocument, ReviewMarkers, ReviewMeta, ReviewMetaEntry,
    ReviewMetaPatch, ReviewSuggestionKind, ReviewSuggestionMarker, has_review_endmatter,
    hydrate_inline_comment_bodies, inline_comment_body, parse_review_document,
    review_block_to_nodes, review_blocks_to_nodes, review_markdown_to_nodes, review_markers,
    review_meta_with_inline_comment_bodies, split_review_endmatter,
};
pub use rows::{
    BlockRow, LinkRange, MarkRun, block_rows_to_markdown, block_rows_to_nodes, is_utf16_boundary,
    markdown_to_block_rows, utf16_len,
};
pub use text_diff::{
    MULTI_HUNK_CHAR_LIMIT, TextDiff, utf16_text_diff, utf16_text_diff_hunks,
    utf16_text_diff_hunks_bounded,
};
pub use trailing::{is_empty_paragraph, strip_trailing_empty_paragraphs};

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("unsupported Markdown construct: {0}")]
pub struct Unsupported(pub String);

impl Unsupported {
    pub fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }

    pub fn context(self, context: impl AsRef<str>) -> Self {
        Self(format!("{}: {}", context.as_ref(), self.0))
    }
}
