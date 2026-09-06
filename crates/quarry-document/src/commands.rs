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
    DeleteBlock {
        block: String,
    },
    SetBlock {
        block: String,
        kind: String,
        attrs: BTreeMap<String, serde_json::Value>,
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
        )
    }
}

impl Document {
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
        self.validate_command_base(&branch, commands)?;
        branch.apply(commands)?;
        // An ID added independently after the supplied base cannot be reused.
        for command in commands {
            if let Command::AddComment { id, .. } = command {
                self.require_new(crate::COMMENTS, id)?;
            }
        }
        self.merge(&branch)
    }

    /// An intervening keystroke does not make a split or move ambiguous.
    /// Competing structural changes require a fresh read. Decisions also check
    /// intervening review edits, and validate their targets on current text.
    pub(crate) fn validate_command_base(&self, base: &Self, commands: &[Command]) -> Result<()> {
        for command in commands {
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

    pub(crate) fn apply_one(&mut self, command: &Command) -> Result<()> {
        match command {
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
            Command::DeleteBlock { block } => self.delete_block(block),
            Command::SetBlock { block, kind, attrs } => self.set_block(block, kind, attrs.clone()),
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
