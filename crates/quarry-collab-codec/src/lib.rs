pub mod markdown;
pub mod normalize;
pub mod review;
pub mod slate;
pub mod trailing;
pub mod yjs_builder;

pub use markdown::{block_markdown_to_slate, block_markdown_to_slate_raw};
pub use review::{
    has_review_endmatter, hydrate_inline_comment_bodies, review_block_to_slate,
    review_blocks_to_slate, review_markdown_to_slate, review_meta_with_inline_comment_bodies,
    split_review_endmatter, ReviewMeta, ReviewMetaEntry, ReviewMetaPatch,
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
