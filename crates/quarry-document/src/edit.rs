//! Content intent is independent of input events, undo groups, and transport.
//! The same interpreter runs in the authority and in the browser's WASM builder.
use crate::{
    Command, Document, DocumentError, Result, SeedBlock, TargetOwner, TextPoint, TextRange,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum EditMode {
    Direct,
    Suggest {
        id: String,
        author: String,
    },
    /// An explicit continuation, never a search for a nearby review item.
    Continue {
        id: String,
        author: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum EditAction {
    InsertText {
        at: TextPoint,
        text: String,
    },
    DeleteText {
        ranges: Vec<TextRange>,
    },
    InsertBlock {
        block: SeedBlock,
    },
    ReplaceText {
        at: TextPoint,
        ranges: Vec<TextRange>,
        text: String,
    },
    Format {
        ranges: Vec<TextRange>,
        name: String,
        value: serde_json::Value,
    },
    SetBlock {
        block: String,
        proposal: Option<String>,
        kind: String,
        attrs: BTreeMap<String, serde_json::Value>,
    },
    MoveBlock {
        block: String,
        parent: Option<String>,
        before: Option<String>,
    },
    DeleteBlock {
        block: String,
    },
    InsertBlocks {
        parent: Option<String>,
        before: Option<String>,
        blocks: Vec<SeedBlock>,
    },
    SplitBlock {
        block: String,
        proposal: Option<String>,
        at: TextPoint,
        new_block: String,
    },
    JoinBlocks {
        left: String,
        right: String,
        proposal: Option<String>,
    },
    SetProposedStructure {
        proposal: String,
        blocks: Vec<crate::ProposedBlockPlacement>,
    },
}

impl Document {
    /// Lower an intent against the version it addresses. Returned primitives
    /// retain native positions and remain subject to normal delayed-write checks.
    /// Planning changes no document state and does not allocate source identities.
    pub fn edit_commands(&self, mode: &EditMode, action: &EditAction) -> Result<Vec<Command>> {
        let suggestion = match mode {
            EditMode::Direct => None,
            EditMode::Suggest { id, author } => {
                self.require_new_review(id)?;
                Some((id, author))
            }
            EditMode::Continue { id, author } => Some((id, author)),
        };
        let unsupported =
            || DocumentError::Invalid("This action cannot continue a suggestion".into());
        if matches!(mode, EditMode::Continue { .. })
            && !matches!(
                action,
                EditAction::ReplaceText { .. }
                    | EditAction::InsertText { .. }
                    | EditAction::DeleteText { .. }
            )
        {
            return Err(unsupported());
        }
        let commands = match action {
            EditAction::InsertText { at, text } => {
                return self.edit_commands(
                    mode,
                    &EditAction::ReplaceText {
                        at: at.clone(),
                        ranges: Vec::new(),
                        text: text.clone(),
                    },
                );
            }
            EditAction::DeleteText { ranges } => {
                if matches!(mode, EditMode::Direct) {
                    return Ok(vec![Command::DeleteText {
                        ranges: ranges.clone(),
                    }]);
                }
                let Some(first) = ranges.first() else {
                    return Ok(Vec::new());
                };
                return self.edit_commands(
                    mode,
                    &EditAction::ReplaceText {
                        at: TextPoint {
                            source: first.source.clone(),
                            cursor: first.start.clone(),
                        },
                        ranges: ranges.clone(),
                        text: String::new(),
                    },
                );
            }
            EditAction::InsertBlock { block } => {
                if suggestion.is_some() {
                    return Err(DocumentError::Invalid(
                        "Stage a proposed block tree before inserting it".into(),
                    ));
                }
                vec![Command::InsertBlock {
                    block: block.clone(),
                }]
            }
            EditAction::ReplaceText { at, ranges, text } => {
                if ranges.is_empty() && text.is_empty() {
                    return Ok(Vec::new());
                }
                // Direct text input stays on the fast source path. There is no
                // whole-document projection or proposal scan for a keypress.
                if matches!(mode, EditMode::Direct) {
                    return Ok(Self::replace_commands(at, ranges, text));
                }
                let location = self.locate_point(at)?.ok_or_else(|| {
                    DocumentError::Conflict("Edit position is no longer visible".into())
                })?;
                let (target, _) = self.capture_target(ranges)?;
                let parts = self.resolve_target(&target)?.attachments;
                match location.owner {
                    TargetOwner::Proposal(id) => {
                        // Selecting proposed text explicitly addresses its owner.
                        // Collaborators edit those characters in place, including
                        // proposals authored by another participant.
                        if parts
                            .iter()
                            .any(|part| part.owner != TargetOwner::Proposal(id.clone()))
                        {
                            return Err(DocumentError::Invalid("Replace text within one suggestion; decide intervening suggestions first".into()));
                        }
                        if let EditMode::Continue { id: expected, .. } = mode
                            && expected != &id
                        {
                            return Err(unsupported());
                        }
                        Self::replace_commands(at, ranges, text)
                    }
                    TargetOwner::Block(block) => {
                        if parts
                            .iter()
                            .any(|part| !matches!(part.owner, TargetOwner::Block(_)))
                        {
                            return Err(DocumentError::Invalid(
                                "Decide intervening suggestions before replacing canonical text"
                                    .into(),
                            ));
                        }
                        let Some((id, author)) = suggestion else {
                            return Err(DocumentError::Invalid("Missing suggestion intent".into()));
                        };
                        match mode {
                            EditMode::Continue { .. } => vec![Command::ContinueTextProposal {
                                id: id.clone(),
                                author: author.clone(),
                                at: at.clone(),
                                ranges: ranges.clone(),
                                text: text.clone(),
                            }],
                            _ => vec![Command::ProposeReplacement {
                                id: id.clone(),
                                author: author.clone(),
                                block,
                                at: at.clone(),
                                ranges: ranges.clone(),
                                text: text.clone(),
                            }],
                        }
                    }
                }
            }
            EditAction::Format {
                ranges,
                name,
                value,
            } => {
                if let Some((id, author)) = suggestion {
                    let mut canonical = Vec::new();
                    let mut proposed = Vec::new();
                    // A range may cross segments that now have different owners.
                    // Resolve it to the same characters, never copied strings.
                    for part in self
                        .resolve_target(&self.capture_target(ranges)?.0)?
                        .attachments
                    {
                        match part.owner {
                            TargetOwner::Block(block) => {
                                canonical.extend(self.selection(&block, part.start, part.end)?)
                            }
                            TargetOwner::Proposal(proposal) => proposed
                                .extend(self.proposal_selection(&proposal, part.start, part.end)?),
                        }
                    }
                    let mut commands = Vec::new();
                    if !canonical.is_empty() {
                        commands.push(Command::ProposeFormat {
                            id: id.clone(),
                            author: author.clone(),
                            ranges: canonical,
                            name: name.clone(),
                            value: value.clone(),
                        });
                    }
                    if !proposed.is_empty() {
                        commands.push(Command::Format {
                            ranges: proposed,
                            name: name.clone(),
                            value: value.clone(),
                        });
                    }
                    commands
                } else {
                    vec![Command::Format {
                        ranges: ranges.clone(),
                        name: name.clone(),
                        value: value.clone(),
                    }]
                }
            }
            EditAction::SetBlock {
                block,
                proposal,
                kind,
                attrs,
            } => {
                if let Some(proposal) = proposal {
                    vec![Command::SetProposedBlock {
                        proposal: proposal.clone(),
                        block: block.clone(),
                        kind: kind.clone(),
                        attrs: attrs.clone(),
                    }]
                } else if let Some((id, author)) = suggestion {
                    vec![Command::ProposeBlockUpdate {
                        id: id.clone(),
                        author: author.clone(),
                        block: block.clone(),
                        kind: kind.clone(),
                        attrs: attrs.clone(),
                    }]
                } else {
                    vec![Command::SetBlock {
                        block: block.clone(),
                        kind: kind.clone(),
                        attrs: attrs.clone(),
                    }]
                }
            }
            EditAction::MoveBlock {
                block,
                parent,
                before,
            } => {
                if let Some((id, author)) = suggestion {
                    vec![Command::ProposeBlockMove {
                        id: id.clone(),
                        author: author.clone(),
                        block: block.clone(),
                        parent: parent.clone(),
                        before: before.clone(),
                    }]
                } else {
                    vec![Command::MoveBlock {
                        block: block.clone(),
                        parent: parent.clone(),
                        before: before.clone(),
                    }]
                }
            }
            EditAction::DeleteBlock { block } => {
                if let Some((id, author)) = suggestion {
                    vec![Command::ProposeBlockDelete {
                        id: id.clone(),
                        author: author.clone(),
                        block: block.clone(),
                    }]
                } else {
                    vec![Command::DeleteBlock {
                        block: block.clone(),
                    }]
                }
            }
            EditAction::InsertBlocks {
                parent,
                before,
                blocks,
            } => {
                if let Some((id, author)) = suggestion {
                    vec![Command::ProposeBlocks {
                        id: id.clone(),
                        author: author.clone(),
                        parent: parent.clone(),
                        before: before.clone(),
                        blocks: blocks.clone(),
                    }]
                } else {
                    let mut commands: Vec<_> = blocks
                        .iter()
                        .map(|block| Command::InsertBlock {
                            block: block.clone(),
                        })
                        .collect();
                    // The external placement applies to the roots of the seed tree.
                    for block in blocks.iter().filter(|block| block.parent.is_none()) {
                        commands.push(Command::MoveBlock {
                            block: block.id.clone(),
                            parent: parent.clone(),
                            before: before.clone(),
                        });
                    }
                    commands
                }
            }
            EditAction::SplitBlock {
                block,
                proposal,
                at,
                new_block,
            } => {
                if let Some(proposal) = proposal {
                    vec![Command::SplitProposedBlock {
                        proposal: proposal.clone(),
                        block: block.clone(),
                        at: at.clone(),
                        new_block: new_block.clone(),
                    }]
                } else if suggestion.is_some() {
                    return Err(DocumentError::Invalid(
                        "Splitting canonical blocks in Suggesting mode is not supported".into(),
                    ));
                } else {
                    vec![Command::SplitBlock {
                        block: block.clone(),
                        at: at.clone(),
                        new_block: new_block.clone(),
                    }]
                }
            }
            EditAction::JoinBlocks {
                left,
                right,
                proposal,
            } => {
                if let Some(proposal) = proposal {
                    vec![Command::JoinProposedBlocks {
                        proposal: proposal.clone(),
                        left: left.clone(),
                        right: right.clone(),
                    }]
                } else if suggestion.is_some() {
                    return Err(DocumentError::Invalid(
                        "Joining canonical blocks in Suggesting mode is not supported".into(),
                    ));
                } else {
                    vec![Command::JoinBlocks {
                        left: left.clone(),
                        right: right.clone(),
                    }]
                }
            }
            EditAction::SetProposedStructure { proposal, blocks } => {
                vec![Command::SetProposedStructure {
                    proposal: proposal.clone(),
                    blocks: blocks.clone(),
                }]
            }
        };
        Ok(commands)
    }

    fn replace_commands(at: &TextPoint, ranges: &[TextRange], text: &str) -> Vec<Command> {
        let mut commands = Vec::new();
        if !ranges.is_empty() {
            commands.push(Command::DeleteText {
                ranges: ranges.to_vec(),
            });
        }
        if !text.is_empty() {
            commands.push(Command::InsertText {
                at: at.clone(),
                text: text.into(),
            });
        }
        commands
    }
}
