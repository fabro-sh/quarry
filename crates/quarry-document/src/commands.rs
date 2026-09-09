#![allow(
    clippy::large_stack_arrays,
    reason = "utoipa expands the command schema into a fixed local array"
)]

use crate::{Document, DocumentError, Result, SeedBlock, TextPoint, TextRange};
use automerge::{
    ChangeHash, ScalarValue,
    marks::{ExpandMark, Mark},
    transaction::Transactable,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Transport-independent commands. Positions belong to a document version;
/// callers must retain that version when submitting delayed text/review edits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum Command {
    Edit {
        mode: crate::EditMode,
        action: crate::EditAction,
    },
    Revert {
        before: Vec<String>,
        after: Vec<String>,
    },
    InsertBlock {
        block: SeedBlock,
    },
    MoveBlock {
        block: String,
        parent: Option<String>,
        before: Option<String>,
    },
    SplitBlock {
        block: String,
        at: TextPoint,
        new_block: String,
    },
    JoinBlocks {
        left: String,
        right: String,
    },
    JoinContainers {
        left: String,
        right: String,
    },
    CutSelection {
        transfer: String,
        anchor: TextPoint,
        focus: TextPoint,
    },
    PasteCut {
        transfer: String,
        at: TextPoint,
        new_block: String,
    },
    MoveText {
        block: String,
        proposal: Option<String>,
        start: TextPoint,
        end: TextPoint,
        to: TextPoint,
    },
    DeleteBlock {
        block: String,
    },
    SetBlock {
        block: String,
        kind: String,
        attrs: BTreeMap<String, serde_json::Value>,
    },
    ConvertBlock {
        block: String,
        target: crate::BlockConversion,
    },
    ConvertProposedBlock {
        proposal: String,
        block: String,
        target: crate::BlockConversion,
    },
    ProposeBlockConversion {
        id: String,
        author: String,
        block: String,
        target: crate::BlockConversion,
    },
    InsertText {
        at: TextPoint,
        text: String,
    },
    DeleteText {
        ranges: Vec<TextRange>,
    },
    Format {
        ranges: Vec<TextRange>,
        name: String,
        value: serde_json::Value,
    },
    AddComment {
        id: String,
        author: String,
        body: String,
        ranges: Vec<TextRange>,
    },
    ReplyComment {
        id: String,
        parent: String,
        author: String,
        body: String,
    },
    EditComment {
        id: String,
        body: String,
    },
    DeleteComment {
        id: String,
    },
    ResolveComment {
        id: String,
        resolved: bool,
    },
    ProposeInsertion {
        id: String,
        author: String,
        block: String,
        at: TextPoint,
        text: String,
    },
    ProposeReplacement {
        id: String,
        author: String,
        block: String,
        at: TextPoint,
        ranges: Vec<TextRange>,
        text: String,
    },
    ProposeFormat {
        id: String,
        author: String,
        ranges: Vec<TextRange>,
        name: String,
        value: serde_json::Value,
    },
    ContinueTextProposal {
        id: String,
        author: String,
        at: TextPoint,
        ranges: Vec<TextRange>,
        text: String,
    },
    ProposeBlockUpdate {
        id: String,
        author: String,
        block: String,
        kind: String,
        attrs: BTreeMap<String, serde_json::Value>,
    },
    ProposeBlockMove {
        id: String,
        author: String,
        block: String,
        parent: Option<String>,
        before: Option<String>,
    },
    ProposeBlockSplit {
        id: String,
        author: String,
        block: String,
        at: TextPoint,
        new_block: String,
    },
    ProposeBlockPaste {
        id: String,
        author: String,
        block: String,
        at: TextPoint,
        focus: TextPoint,
        blocks: Vec<SeedBlock>,
    },
    ProposeBlockJoin {
        id: String,
        author: String,
        left: String,
        right: String,
    },
    ProposeBlockDelete {
        id: String,
        author: String,
        block: String,
    },
    ProposeBlocks {
        id: String,
        author: String,
        parent: Option<String>,
        before: Option<String>,
        blocks: Vec<SeedBlock>,
    },
    SetProposedBlock {
        proposal: String,
        block: String,
        kind: String,
        attrs: BTreeMap<String, serde_json::Value>,
    },
    SetProposedStructure {
        proposal: String,
        blocks: Vec<crate::ProposedBlockPlacement>,
    },
    SplitProposedBlock {
        proposal: String,
        block: String,
        at: TextPoint,
        new_block: String,
    },
    JoinProposedBlocks {
        proposal: String,
        left: String,
        right: String,
    },
    AcceptProposal {
        id: String,
    },
    RejectProposal {
        id: String,
    },
    EditProposal {
        id: String,
        body: String,
    },
    AddConflict {
        conflict: crate::Conflict,
    },
    ResolveConflict {
        id: String,
    },
}

impl Command {
    /// These commands retain their native meaning across structural changes.
    pub(crate) fn supports_delayed_merge(&self) -> bool {
        matches!(
            self,
            Self::InsertText { .. }
                | Self::DeleteText { .. }
                | Self::Format { .. }
                | Self::AddComment { .. }
                | Self::ContinueTextProposal { .. }
                | Self::ProposeInsertion { .. }
                | Self::ProposeReplacement { .. }
                | Self::ProposeFormat { .. }
        )
    }
}

impl Document {
    /// Execute on the original version while collecting the native operations
    /// used for delayed-write validation. Expansion is sequential, so later
    /// intents can address sources created earlier in the same request.
    pub(crate) fn apply_expanded(&mut self, commands: &[Command]) -> Result<Vec<Command>> {
        self.atomic(|candidate| {
            let mut expanded = Vec::new();
            for command in commands {
                let lowered = match command {
                    Command::Edit { mode, action } => candidate.edit_commands(mode, action)?,
                    command => vec![command.clone()],
                };
                for command in lowered {
                    candidate.apply_one(&command)?;
                    expanded.push(command);
                }
            }
            Ok(expanded)
        })
    }
    /// All-or-nothing batch; no earlier command leaks when a later one fails.
    pub fn apply(&mut self, commands: &[Command]) -> Result<()> {
        self.atomic(|candidate| {
            for command in commands {
                candidate.apply_one(command)?;
            }
            Ok(())
        })
    }

    /// Construct text operations against the version the caller actually saw.
    /// This preserves native mark boundary rules when the comment arrives late.
    /// Generic structural rebasing is deliberately not inferred from text.
    pub fn apply_at(&mut self, base: &[ChangeHash], commands: &[Command]) -> Result<()> {
        let mut expected = base.to_vec();
        expected.sort();
        let mut current = self.heads();
        current.sort();
        if expected == current {
            return self.apply(commands);
        }
        if base.is_empty() {
            return Err(DocumentError::Conflict("Missing document version".into()));
        }
        let mut branch = self.fork_at(base)?;
        let original = branch.fork();
        let expanded = branch.apply_expanded(commands)?;
        self.validate_command_base(&original, &expanded)?;
        // An ID added independently after the supplied base cannot be reused.
        for command in commands {
            if let Command::AddComment { id, .. } = command {
                self.require_new(crate::COMMENTS, id)?;
            }
        }
        self.merge_edited_branch(&branch, &expanded)
    }

    /// An intervening keystroke does not make a split or move ambiguous.
    /// Competing structural changes require a fresh read. Decisions also check
    /// intervening review edits, and validate their targets on current text.
    pub(crate) fn validate_command_base(&self, base: &Self, commands: &[Command]) -> Result<()> {
        for command in commands {
            if let Command::ContinueTextProposal { id, ranges, .. } = command {
                if let Ok(original) = base.proposal(id) {
                    if self.proposal(id)? != original {
                        return Err(DocumentError::Conflict(
                            "Suggestion changed; read it again before continuing".into(),
                        ));
                    }
                    self.validate_proposal_acceptance(id)?;
                } else {
                    self.require_new_review(id)?;
                }
                self.validate_review_ranges(base, ranges)?;
            }
            match command {
                Command::ProposeInsertion { id, .. }
                | Command::ProposeReplacement { id, .. }
                | Command::ProposeFormat { id, .. } => self.require_new_review(id)?,
                _ => {}
            }
            match command {
                Command::ProposeReplacement { ranges, .. }
                | Command::ProposeFormat { ranges, .. } => {
                    self.validate_review_ranges(base, ranges)?
                }
                _ => {}
            }
            if let Command::InsertText { at, .. } = command
                && base.locate_point(at).ok().flatten().is_some()
                && self.locate_point(at)?.is_none()
            {
                return Err(DocumentError::Conflict(
                    "Text target is no longer visible; recover this edit before resubmitting"
                        .into(),
                ));
            }
        }
        if commands.iter().all(Command::supports_delayed_merge) {
            return Ok(());
        }
        if self.records::<crate::Block>(crate::BLOCKS)?
            != base.records::<crate::Block>(crate::BLOCKS)?
            || self.proposals()? != base.proposals()?
        {
            return Err(DocumentError::Conflict(
                "Document structure changed; read it again before this command".into(),
            ));
        }
        for command in commands {
            // Deletion hides an entire subtree, including text and formatting
            // that the caller may never have seen. Splits and moves preserve it.
            if let Command::DeleteBlock { block } = command
                && base.block(block).is_ok()
                && self.subtree_content(block)? != base.subtree_content(block)?
            {
                return Err(DocumentError::Conflict(
                    "Block content changed; read it again before deleting it".into(),
                ));
            }
            let changed = match command {
                Command::ResolveComment { id, .. }
                | Command::EditComment { id, .. }
                | Command::DeleteComment { id } => self.comment(id)? != base.comment(id)?,
                Command::ResolveConflict { id } => self.conflict(id)? != base.conflict(id)?,
                _ => false,
            };
            if changed {
                return Err(DocumentError::Conflict(
                    "The review record changed; read it again before this decision".into(),
                ));
            }
        }
        // In particular, accepting an old replacement must not silently delete
        // text that changed since it was proposed. This candidate is discarded.
        self.fork().apply(commands)
    }

    fn validate_review_ranges(&self, base: &Self, ranges: &[TextRange]) -> Result<()> {
        for range in ranges {
            // References created earlier in this same request do not exist in
            // its base. Their final targets are checked after the private merge.
            if let Ok(original) = base.capture_target(std::slice::from_ref(range))
                && self.capture_target(std::slice::from_ref(range))? != original
            {
                return Err(DocumentError::Conflict(
                    "Suggestion target changed; read it again before proposing".into(),
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn merge_edited_branch(
        &mut self,
        branch: &Self,
        commands: &[Command],
    ) -> Result<()> {
        let mut candidate = self.fork();
        candidate.merge(branch)?;
        for command in commands {
            let id = match command {
                Command::ProposeInsertion { id, .. }
                | Command::ProposeReplacement { id, .. }
                | Command::ProposeFormat { id, .. }
                | Command::ContinueTextProposal { id, .. } => id,
                _ => continue,
            };
            // A decision later in this request may already have closed it.
            // Verify every proposal that was valid on the declared base remains
            // valid with concurrent work. Failed merges publish nothing.
            if branch.validate_proposal_acceptance(id).is_ok() {
                candidate.validate_proposal_acceptance(id)?;
            }
        }
        *self = candidate;
        Ok(())
    }

    pub(crate) fn apply_one(&mut self, command: &Command) -> Result<()> {
        match command {
            Command::Edit { mode, action } => {
                for command in self.edit_commands(mode, action)? {
                    self.apply_one(&command)?;
                }
                Ok(())
            }
            Command::ContinueTextProposal {
                id,
                author,
                at,
                ranges,
                text,
            } => self.continue_text_proposal(id, author, at, ranges, text),
            Command::Revert { before, after } => {
                let parse = |heads: &[String]| {
                    heads
                        .iter()
                        .map(|h| h.parse())
                        .collect::<std::result::Result<Vec<_>, _>>()
                        .map_err(|_| DocumentError::Invalid("Invalid undo version".into()))
                };
                self.revert(&parse(before)?, &parse(after)?)
            }
            Command::InsertBlock { block } => self.insert_block(block.clone()),
            Command::MoveBlock {
                block,
                parent,
                before,
            } => self.move_block(block, parent.clone(), before.as_deref()),
            Command::SplitBlock {
                block,
                at,
                new_block,
            } => self.split_block(block, at, new_block),
            Command::JoinBlocks { left, right } => self.join_blocks(left, right),
            Command::JoinContainers { left, right } => {
                let commands = self.join_container_commands(left, right)?;
                self.apply(&commands)
            }
            Command::CutSelection {
                transfer,
                anchor,
                focus,
            } => self.cut_selection(transfer, anchor, focus),
            Command::PasteCut {
                transfer,
                at,
                new_block,
            } => self.paste_cut(transfer, at, new_block),
            Command::MoveText {
                block,
                proposal,
                start,
                end,
                to,
            } => self.move_text(block, proposal.as_deref(), start, end, to),
            Command::DeleteBlock { block } => self.delete_block(block),
            Command::SetBlock { block, kind, attrs } => self.set_block(block, kind, attrs.clone()),
            Command::ConvertBlock { block, target } => self.convert_block(block, target),
            Command::ConvertProposedBlock {
                proposal,
                block,
                target,
            } => self.convert_proposed_block(proposal, block, target),
            Command::ProposeBlockConversion {
                id,
                author,
                block,
                target,
            } => self.propose_block_conversion(id, author, block, target),
            Command::InsertText { at, text } => self.insert_text(at, text),
            Command::DeleteText { ranges } => self.delete_text(ranges),
            Command::Format {
                ranges,
                name,
                value,
            } => self.format(ranges, name, value),
            Command::AddComment {
                id,
                author,
                body,
                ranges,
            } => self.add_comment(id, author, body, ranges),
            Command::ReplyComment {
                id,
                parent,
                author,
                body,
            } => self.reply_comment(id, parent, author, body),
            Command::EditComment { id, body } => self.edit_comment(id, body),
            Command::DeleteComment { id } => self.delete_comment(id),
            Command::ResolveComment { id, resolved } => self.resolve_comment(id, *resolved),
            Command::ProposeInsertion {
                id,
                author,
                block,
                at,
                text,
            } => self.propose_insertion(id, author, block, at, text),
            Command::ProposeReplacement {
                id,
                author,
                block,
                at,
                ranges,
                text,
            } => self.propose_replacement(id, author, block, at, ranges, text),
            Command::ProposeFormat {
                id,
                author,
                ranges,
                name,
                value,
            } => self.propose_format(id, author, ranges, name, value),
            Command::ProposeBlockUpdate {
                id,
                author,
                block,
                kind,
                attrs,
            } => self.propose_block_update(id, author, block, kind, attrs.clone()),
            Command::ProposeBlockMove {
                id,
                author,
                block,
                parent,
                before,
            } => self.propose_block_move(id, author, block, parent.clone(), before.clone()),
            Command::ProposeBlockSplit {
                id,
                author,
                block,
                at,
                new_block,
            } => self.propose_block_split(id, author, block, at, new_block),
            Command::ProposeBlockPaste {
                id,
                author,
                block,
                at,
                focus,
                blocks,
            } => self.propose_block_paste(id, author, block, at, focus, blocks),
            Command::ProposeBlockJoin {
                id,
                author,
                left,
                right,
            } => self.propose_block_join(id, author, left, right),
            Command::ProposeBlockDelete { id, author, block } => {
                self.propose_block_delete(id, author, block)
            }
            Command::ProposeBlocks {
                id,
                author,
                parent,
                before,
                blocks,
            } => self.propose_blocks(id, author, parent.clone(), before.clone(), blocks),
            Command::SetProposedBlock {
                proposal,
                block,
                kind,
                attrs,
            } => self.set_proposed_block(proposal, block, kind, attrs.clone()),
            Command::SetProposedStructure { proposal, blocks } => {
                self.set_proposed_structure(proposal, blocks)
            }
            Command::SplitProposedBlock {
                proposal,
                block,
                at,
                new_block,
            } => self.split_proposed_block(proposal, block, at, new_block),
            Command::JoinProposedBlocks {
                proposal,
                left,
                right,
            } => self.join_proposed_blocks(proposal, left, right),
            Command::AcceptProposal { id } => self.accept_proposal(id),
            Command::RejectProposal { id } => self.reject_proposal(id),
            Command::EditProposal { id, body } => self.edit_proposal(id, body),
            Command::AddConflict { conflict } => self.add_conflict(conflict.clone()),
            Command::ResolveConflict { id } => self.resolve_conflict(id),
        }
    }

    pub fn format(
        &mut self,
        ranges: &[TextRange],
        name: &str,
        value: &serde_json::Value,
    ) -> Result<()> {
        crate::schema::validate_format(name, value)?;
        let scalar = match value {
            serde_json::Value::Null => ScalarValue::Null,
            serde_json::Value::Bool(v) => ScalarValue::Boolean(*v),
            serde_json::Value::String(v) => ScalarValue::Str(v.as_str().into()),
            serde_json::Value::Number(v) if v.is_i64() => ScalarValue::Int(
                v.as_i64()
                    .ok_or_else(|| DocumentError::Invalid("Invalid mark integer".into()))?,
            ),
            serde_json::Value::Number(v) if v.is_u64() => ScalarValue::Uint(
                v.as_u64()
                    .ok_or_else(|| DocumentError::Invalid("Invalid mark integer".into()))?,
            ),
            _ => {
                return Err(DocumentError::Invalid(
                    "Formatting values must be scalar strings, booleans, integers, or null".into(),
                ));
            }
        };
        self.atomic(|d| {
            for range in ranges {
                let (start, end) = d.range_offsets(range)?;
                let expand = if matches!(name, "link" | "wikilink") {
                    ExpandMark::None
                } else {
                    ExpandMark::Both
                };
                d.invalidate_source(&range.source);
                d.crdt.mark(
                    d.source(&range.source)?,
                    Mark::new(name.to_string(), scalar.clone(), start, end),
                    expand,
                )?;
            }
            Ok(())
        })
    }
}
