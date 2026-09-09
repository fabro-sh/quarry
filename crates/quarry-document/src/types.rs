use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The format is deliberately independent of an editor's node representation.
pub const SCHEMA_VERSION: u64 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct SegmentRef {
    pub source: String,
    pub segment: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Block {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub attrs: BTreeMap<String, serde_json::Value>,
    pub parent: Option<String>,
    pub position: usize,
    pub segments: Vec<SegmentRef>,
    pub deleted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct SeedBlock {
    pub id: String,
    pub kind: String,
    pub parent: Option<String>,
    pub position: usize,
    #[serde(default)]
    pub attrs: BTreeMap<String, serde_json::Value>,
    pub text: String,
}

/// A proposed tree can retain existing characters or introduce a new block.
/// Existing blocks never take replacement text from an editor projection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum ProposedBlockPlacement {
    Existing {
        block: String,
        parent: Option<String>,
        position: usize,
    },
    New {
        block: SeedBlock,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct TextPoint {
    pub source: String,
    pub cursor: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct TextRange {
    pub source: String,
    pub start: String,
    pub end: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct TargetFragment {
    pub source: String,
    /// Exact native character identities. Retained text makes these durable
    /// even when the characters are hidden by a deletion or their owner moves.
    pub first: String,
    pub last: String,
    pub last_width: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum DiscussionState {
    Open,
    Resolved,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub body: String,
    pub target: Vec<TargetFragment>,
    pub original_quote: String,
    pub state: DiscussionState,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub metadata: ReviewMetadata,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ReviewMetadata {
    pub created_at: String,
    pub updated_at: String,
    /// Preserves records whose targets were already missing before migration.
    #[serde(default)]
    pub unattached_reason: Option<String>,
    /// Exact legacy fields retained by the one-time importer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_record: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum ProposalState {
    Open,
    Accepted,
    Rejected,
    /// Legacy storage did not distinguish accepted from rejected decisions.
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Proposal {
    pub id: String,
    pub author: String,
    pub body: String,
    pub action: ProposalAction,
    pub segments: Vec<SegmentRef>,
    pub state: ProposalState,
    pub metadata: ReviewMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum ProposalAction {
    Unavailable {
        reason: String,
    },
    Text {
        at: TextPoint,
        delete_target: Vec<TargetFragment>,
        original_quote: String,
    },
    Format {
        target: Vec<TargetFragment>,
        name: String,
        value: serde_json::Value,
        /// Text and this mark's values at proposal time. Other marks and
        /// block placement can change without invalidating the decision.
        expected: Vec<(String, serde_json::Value)>,
    },
    UpdateBlock {
        block: String,
        block_kind: String,
        attrs: BTreeMap<String, serde_json::Value>,
        expected_kind: String,
        expected_attrs: BTreeMap<String, serde_json::Value>,
    },
    ConvertBlock {
        block: String,
        target: crate::BlockConversion,
        expected: Vec<Block>,
    },
    MoveBlock {
        block: String,
        parent: Option<String>,
        before: Option<String>,
        expected_parent: Option<String>,
        expected_before: Option<String>,
    },
    SplitBlock {
        block: String,
        at: TextPoint,
        new_block: String,
    },
    /// Insert copied sibling text blocks at a canonical text position. The
    /// proposal owns the copied characters; acceptance moves them into the
    /// split destination without copying their native identities again.
    PasteBlocks {
        block: String,
        at: TextPoint,
        delete_target: Vec<TargetFragment>,
        original_quote: String,
        delete_ranges: Vec<TextRange>,
        delete_blocks: Vec<PasteDeletedBlock>,
        joins: Vec<PasteBlockJoin>,
        blocks: Vec<Block>,
    },
    JoinBlocks {
        left: String,
        right: String,
    },
    DeleteBlock {
        block: String,
        /// Exact subtree content when the deletion was proposed. A changed
        /// subtree requires a new decision instead of dropping unseen edits.
        expected: String,
    },
    InsertBlocks {
        parent: Option<String>,
        before: Option<String>,
        blocks: Vec<Block>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct PasteBlockJoin {
    pub left: String,
    pub right: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct PasteDeletedBlock {
    pub block: String,
    pub expected: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum TargetOwner {
    Block(String),
    Proposal(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Attachment {
    pub owner: TargetOwner,
    /// UTF-16 offsets into the displayed block or proposal, excluding markers.
    pub start: usize,
    pub end: usize,
    pub quote: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum TargetState {
    Attached,
    /// The characters remain in a removed block. The discussion still exists.
    Hidden,
    Deleted,
    /// The imported record never had a verified native target.
    Unattached,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ResolvedTarget {
    pub state: TargetState,
    pub attachments: Vec<Attachment>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct TextRun {
    pub text: String,
    pub marks: BTreeMap<String, serde_json::Value>,
    pub source: String,
    pub source_start: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct BlockView {
    pub block: Block,
    pub text: String,
    pub runs: Vec<TextRun>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DocumentView {
    pub document_id: String,
    pub schema_version: u64,
    pub heads: Vec<String>,
    pub blocks: Vec<BlockView>,
    pub comments: Vec<CommentView>,
    pub proposals: Vec<ProposalView>,
    pub conflicts: Vec<Conflict>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Conflict {
    pub id: String,
    pub after: Option<String>,
    pub base: String,
    pub incoming: String,
    pub canonical: String,
    pub resolved: bool,
    pub author: String,
    pub metadata: ReviewMetadata,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct CommentView {
    pub comment: Comment,
    pub target: ResolvedTarget,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ProposalView {
    pub proposal: Proposal,
    pub blocks: Vec<BlockView>,
    pub text: String,
    pub runs: Vec<TextRun>,
    pub target: ResolvedTarget,
    /// None only when the current document can accept this proposal.
    pub acceptance_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ResolvedPoint {
    pub owner: TargetOwner,
    pub offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed_block: Option<ResolvedProposedPoint>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ResolvedProposedPoint {
    pub block: String,
    pub offset: usize,
}
