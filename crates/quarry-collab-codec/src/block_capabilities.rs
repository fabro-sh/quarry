//! Shared block vocabulary and behavioral capabilities.
//!
//! The manifest is also imported directly by the browser. Adding a block type
//! therefore requires one capability record instead of synchronized Rust and
//! TypeScript allowlists.

use std::collections::HashSet;
use std::sync::OnceLock;

use serde::Deserialize;

/// How a block stores its document content.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum BlockContentModel {
    /// Flat text plus inline marks and links.
    Text,
    /// Nested block children.
    Container,
    /// Attributes on an otherwise empty element.
    Void,
    /// Verbatim Markdown stored in an attribute.
    Raw,
}

/// Whether text-looking content should be interpreted as inline syntax.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum InlineSyntax {
    /// Inline syntax is parsed into editor nodes and marks.
    Parsed,
    /// Content is preserved literally, as in code.
    Literal,
}

/// Capabilities for one supported Slate block type.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
#[serde(rename_all = "camelCase")]
pub struct BlockCapabilities {
    /// Slate element type and block API vocabulary value.
    #[serde(rename = "type")]
    pub block_type: String,
    /// Where the block stores its document content.
    pub content: BlockContentModel,
    /// Whether inline syntax is parsed or treated literally.
    pub inline_syntax: InlineSyntax,
    /// Whether legacy full-text CriticMarkup can safely represent block deletion.
    pub promote_full_text_delete: bool,
}

fn registry() -> &'static [BlockCapabilities] {
    static REGISTRY: OnceLock<Vec<BlockCapabilities>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let entries: Vec<BlockCapabilities> =
            serde_json::from_str(include_str!("../block-capabilities.json"))
                .expect("block capability manifest must be valid JSON");
        let unique: HashSet<&str> = entries
            .iter()
            .map(|entry| entry.block_type.as_str())
            .collect();
        assert_eq!(
            unique.len(),
            entries.len(),
            "block capability manifest must not contain duplicate types"
        );
        entries
    })
}

/// Returns the registered capabilities for `block_type`.
pub fn block_capabilities(block_type: &str) -> Option<&'static BlockCapabilities> {
    registry()
        .iter()
        .find(|entry| entry.block_type == block_type)
}

/// Returns whether `block_type` belongs to the supported block vocabulary.
pub fn is_known_block_type(block_type: &str) -> bool {
    block_capabilities(block_type).is_some()
}

/// Iterates through supported block types in their public API display order.
pub fn known_block_types() -> impl ExactSizeIterator<Item = &'static str> {
    registry().iter().map(|entry| entry.block_type.as_str())
}

/// Returns whether a block's inline-looking content must remain literal.
pub fn uses_literal_inline_syntax(block_type: &str) -> bool {
    block_capabilities(block_type).is_some_and(|entry| entry.inline_syntax == InlineSyntax::Literal)
}

/// Returns whether a block stores flat text, marks, and links.
pub fn carries_inline_content(block_type: &str) -> bool {
    block_capabilities(block_type).is_some_and(|entry| entry.content == BlockContentModel::Text)
}

/// Returns whether legacy full-text deletion markup can become a block delete.
pub fn can_promote_full_text_delete(block_type: &str) -> bool {
    block_capabilities(block_type).is_some_and(|entry| entry.promote_full_text_delete)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_defines_the_complete_block_vocabulary_in_api_order() {
        assert_eq!(
            known_block_types().collect::<Vec<_>>(),
            vec![
                "p",
                "h1",
                "h2",
                "h3",
                "h4",
                "h5",
                "h6",
                "blockquote",
                "code_block",
                "code_line",
                "mermaid",
                "table",
                "tr",
                "th",
                "td",
                "img",
                "hr",
                "raw_markdown",
            ]
        );
    }

    #[test]
    fn capabilities_distinguish_text_containers_voids_and_literal_content() {
        assert_eq!(
            block_capabilities("p").map(|entry| entry.content),
            Some(BlockContentModel::Text)
        );
        assert_eq!(
            block_capabilities("table").map(|entry| entry.content),
            Some(BlockContentModel::Container)
        );
        assert_eq!(
            block_capabilities("hr").map(|entry| entry.content),
            Some(BlockContentModel::Void)
        );
        assert_eq!(
            block_capabilities("raw_markdown").map(|entry| entry.content),
            Some(BlockContentModel::Raw)
        );
        assert!(uses_literal_inline_syntax("code_line"));
        assert!(!uses_literal_inline_syntax("a"));
        assert!(carries_inline_content("code_line"));
        assert!(!carries_inline_content("mermaid"));
        assert!(can_promote_full_text_delete("blockquote"));
        assert!(!can_promote_full_text_delete("img"));
        assert!(!is_known_block_type("future_block"));
    }
}
