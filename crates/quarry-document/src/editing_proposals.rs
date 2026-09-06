//! Ordered structural edits within an existing block proposal.
use crate::{
    Block, Command, Document, DocumentError, ProposalAction, ProposalState, ProposedBlockPlacement,
    Result, SeedBlock,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum ProposedBlockEdit {
    Insert {
        parent: Option<String>,
        before: Option<String>,
        blocks: Vec<SeedBlock>,
    },
    Move {
        block: String,
        parent: Option<String>,
        before: Option<String>,
    },
    Delete {
        block: String,
    },
    SplitContainer {
        block: String,
        at: usize,
        new_block: String,
    },
    JoinContainers {
        left: String,
        right: String,
    },
}

fn find<'a>(blocks: &'a [Block], id: &str) -> Result<&'a Block> {
    blocks.iter().find(|b| b.id == id).ok_or_else(|| {
        DocumentError::Conflict(format!("Proposed block is no longer present: {id}"))
    })
}

fn move_block(
    blocks: &mut [Block],
    id: &str,
    parent: Option<String>,
    before: Option<&str>,
) -> Result<()> {
    find(blocks, id)?;
    if before == Some(id) {
        return Ok(());
    }
    if let Some(parent) = &parent {
        find(blocks, parent)?;
    }
    let mut siblings: Vec<_> = blocks
        .iter()
        .filter(|b| b.parent == parent && b.id != id)
        .map(|b| (b.position, b.id.clone()))
        .collect();
    siblings.sort();
    let index = match before {
        Some(before) => siblings
            .iter()
            .position(|(_, id)| id == before)
            .ok_or_else(|| {
                DocumentError::Conflict("Proposed move destination is not a sibling".into())
            })?,
        None => siblings.len(),
    };
    siblings.insert(index, (0, id.into()));
    for (position, (_, id)) in siblings.into_iter().enumerate() {
        if let Some(block) = blocks.iter_mut().find(|b| b.id == id) {
            block.parent = parent.clone();
            block.position = position;
        }
    }
    Ok(())
}

impl Document {
    pub(crate) fn edit_proposed_blocks_commands(
        &self,
        id: &str,
        action: &ProposedBlockEdit,
    ) -> Result<Vec<Command>> {
        let proposal = self.proposal(id)?;
        if proposal.state != ProposalState::Open {
            return Err(DocumentError::Conflict("Proposal already decided".into()));
        }
        let ProposalAction::InsertBlocks { mut blocks, .. } = proposal.action else {
            return Err(DocumentError::Invalid(
                "Proposal does not contain blocks".into(),
            ));
        };
        let mut added: Vec<SeedBlock> = Vec::new();
        match action {
            ProposedBlockEdit::Insert {
                parent,
                before,
                blocks: seeds,
            } => {
                for seed in seeds {
                    if blocks.iter().any(|b| b.id == seed.id) {
                        return Err(DocumentError::Invalid(
                            "Duplicate proposed block identity".into(),
                        ));
                    }
                    blocks.push(Block {
                        id: seed.id.clone(),
                        kind: seed.kind.clone(),
                        attrs: seed.attrs.clone(),
                        parent: seed.parent.clone(),
                        position: seed.position,
                        segments: Vec::new(),
                        deleted: false,
                    });
                    added.push(seed.clone());
                }
                let mut roots: Vec<_> = seeds.iter().filter(|b| b.parent.is_none()).collect();
                roots.sort_by_key(|b| b.position);
                for root in roots {
                    move_block(&mut blocks, &root.id, parent.clone(), before.as_deref())?;
                }
            }
            ProposedBlockEdit::Move {
                block,
                parent,
                before,
            } => move_block(&mut blocks, block, parent.clone(), before.as_deref())?,
            ProposedBlockEdit::Delete { block } => {
                find(&blocks, block)?;
                let mut removed = std::collections::BTreeSet::from([block.clone()]);
                loop {
                    let count = removed.len();
                    for b in &blocks {
                        if b.parent.as_ref().is_some_and(|p| removed.contains(p)) {
                            removed.insert(b.id.clone());
                        }
                    }
                    if removed.len() == count {
                        break;
                    }
                }
                blocks.retain(|b| !removed.contains(&b.id));
            }
            ProposedBlockEdit::SplitContainer {
                block,
                at,
                new_block,
            } => {
                let original = find(&blocks, block)?.clone();
                if !crate::block_capabilities(&original.kind)
                    .is_some_and(|c| c.content == crate::BlockContentModel::Container)
                {
                    return Err(DocumentError::Invalid(
                        "Container split requires a container".into(),
                    ));
                }
                if blocks.iter().any(|b| b.id == *new_block) {
                    return Err(DocumentError::Invalid(
                        "Duplicate proposed block identity".into(),
                    ));
                }
                let mut children: Vec<_> = blocks
                    .iter()
                    .filter(|b| b.parent.as_deref() == Some(block))
                    .map(|b| (b.position, b.id.clone()))
                    .collect();
                children.sort();
                if *at > children.len() {
                    return Err(DocumentError::Invalid(
                        "Container split is out of bounds".into(),
                    ));
                }
                for b in &mut blocks {
                    if b.parent == original.parent && b.position > original.position {
                        b.position += 1;
                    }
                }
                let seed = SeedBlock {
                    id: new_block.clone(),
                    kind: original.kind.clone(),
                    attrs: original.attrs.clone(),
                    parent: original.parent.clone(),
                    position: original.position + 1,
                    text: String::new(),
                };
                let mut next = original;
                next.id = new_block.clone();
                next.position += 1;
                next.segments.clear();
                blocks.push(next);
                added.push(seed);
                for (_, child) in &children[*at..] {
                    move_block(&mut blocks, child, Some(new_block.clone()), None)?;
                }
            }
            ProposedBlockEdit::JoinContainers { left, right } => {
                let first = find(&blocks, left)?;
                let second = find(&blocks, right)?;
                if [first, second].iter().any(|b| {
                    !crate::block_capabilities(&b.kind)
                        .is_some_and(|c| c.content == crate::BlockContentModel::Container)
                }) {
                    return Err(DocumentError::Invalid(
                        "Container join requires containers".into(),
                    ));
                }
                let mut siblings: Vec<_> =
                    blocks.iter().filter(|b| b.parent == first.parent).collect();
                siblings.sort_by_key(|b| b.position);
                if !siblings
                    .windows(2)
                    .any(|pair| pair[0].id == *left && pair[1].id == *right)
                {
                    return Err(DocumentError::Conflict(
                        "Join requires adjacent sibling blocks".into(),
                    ));
                }
                let children: Vec<_> = blocks
                    .iter()
                    .filter(|b| b.parent.as_deref() == Some(right))
                    .map(|b| b.id.clone())
                    .collect();
                for child in children {
                    move_block(&mut blocks, &child, Some(left.clone()), None)?;
                }
                blocks.retain(|b| b.id != *right);
            }
        }
        if blocks.is_empty() {
            return Ok(vec![Command::RejectProposal { id: id.into() }]);
        }
        let placements = blocks
            .into_iter()
            .map(|block| {
                if let Some(mut seed) = added.iter().find(|b| b.id == block.id).cloned() {
                    seed.parent = block.parent;
                    seed.position = block.position;
                    ProposedBlockPlacement::New { block: seed }
                } else {
                    ProposedBlockPlacement::Existing {
                        block: block.id,
                        parent: block.parent,
                        position: block.position,
                    }
                }
            })
            .collect();
        Ok(vec![Command::SetProposedStructure {
            proposal: id.into(),
            blocks: placements,
        }])
    }
}
