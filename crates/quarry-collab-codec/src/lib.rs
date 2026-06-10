pub mod markdown;
pub mod markdown_writer;
pub mod normalize;
pub mod reconcile;
pub mod review;
pub mod rows;
pub mod session_doc;
pub mod slate;
pub mod trailing;
pub mod yjs_builder;

pub use markdown::{block_markdown_to_slate, block_markdown_to_slate_raw};
pub use markdown_writer::{is_known_inline_mark, slate_to_markdown};
pub use reconcile::{reconcile, ReconcileBase, ReconcileConflict, ReconcileOp, ReconcileOutcome};
pub use review::{
    has_review_endmatter, hydrate_inline_comment_bodies, inline_comment_body,
    parse_review_document, review_block_to_slate, review_blocks_to_slate, review_markdown_to_slate,
    review_markers, review_meta_with_inline_comment_bodies, split_review_endmatter,
    ReviewCommentMarker, ReviewDocument, ReviewMarkers, ReviewMeta, ReviewMetaEntry,
    ReviewMetaPatch, ReviewSuggestionKind, ReviewSuggestionMarker,
};
pub use rows::{
    block_rows_to_markdown, block_rows_to_nodes, is_utf16_boundary, markdown_to_block_rows,
    utf16_len, BlockRow, LinkRange, MarkRun,
};
pub use session_doc::{
    project_session_nodes, read_review_meta_from_map, reconcile_session_children,
    seed_session_nodes, SessionAnchor, SessionAnchorKind, SessionProjection,
};
pub use slate::{Attrs, Node};
pub use trailing::{is_empty_paragraph, strip_trailing_empty_paragraphs};
pub use yjs_builder::{
    apply_built, apply_review_patch_to_map, build_nodes, encode_update_v1_from_built,
    encode_update_v1_from_built_with_review, write_review_meta_to_map, xmltext_to_slate, BuiltNode,
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
