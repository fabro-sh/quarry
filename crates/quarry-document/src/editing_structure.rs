//! Structural input semantics shared by browser and authority.
use crate::{
    BLOCKS, BlockContentModel, Command, Document, DocumentError, Result, SeedBlock, TargetOwner,
    TextPoint,
};

impl Document {
    /// Apply one visible replacement across canonical and proposed owners.
    /// Existing proposal text changes in place. Canonical text follows the
    /// caller's edit mode. Every command remains in one atomic request.
    pub(crate) fn mixed_replacement_commands(
        &self,
        mode: &crate::EditMode,
        at: &TextPoint,
        text: &str,
        parts: &[crate::Attachment],
    ) -> Result<Vec<Command>> {
        if matches!(mode, crate::EditMode::Continue { .. }) {
            return Err(DocumentError::Invalid(
                "A continued suggestion must stay within one text owner".into(),
            ));
        }
        let location = self
            .locate_point(at)?
            .ok_or_else(|| DocumentError::Conflict("Edit position is no longer visible".into()))?;
        let mut canonical = Vec::new();
        let mut proposed: std::collections::BTreeMap<String, Vec<crate::TextRange>> =
            Default::default();
        let mut removed_split_boundaries = std::collections::BTreeSet::new();
        for part in parts {
            match &part.owner {
                TargetOwner::Block(block) => {
                    canonical.extend(self.selection(block, part.start, part.end)?);
                }
                TargetOwner::Proposal(proposal) => {
                    if part.start == 0
                        && part.end > 0
                        && matches!(
                            self.proposal(proposal)?.action,
                            crate::ProposalAction::SplitBlock { .. }
                        )
                    {
                        removed_split_boundaries.insert(proposal.clone());
                    }
                    proposed
                        .entry(proposal.clone())
                        .or_default()
                        .extend(self.proposal_selection(proposal, part.start, part.end)?);
                }
            }
        }
        let mut commands = Vec::new();
        let mut insertion = at.clone();
        let mut insertion_block = None;
        let mut insertion_proposal = match &location.owner {
            TargetOwner::Proposal(id) => Some(id.clone()),
            TargetOwner::Block(_) => None,
        };
        let mut rejected = std::collections::BTreeSet::new();
        for proposal in proposed.keys() {
            let action = self.proposal(proposal)?.action;
            if removed_split_boundaries.contains(proposal) {
                if insertion_proposal.as_ref() == Some(proposal)
                    && let crate::ProposalAction::SplitBlock { block, at, .. } = action
                {
                    insertion = at;
                    insertion_block = Some(block);
                    insertion_proposal = None;
                }
                rejected.insert(proposal.clone());
                commands.push(Command::RejectProposal {
                    id: proposal.clone(),
                });
            }
        }
        for (proposal, ranges) in &proposed {
            if rejected.contains(proposal) {
                continue;
            }
            if insertion_proposal.as_ref() == Some(proposal) {
                commands.extend(Self::replace_commands(&insertion, ranges, text));
            } else {
                commands.push(Command::DeleteText {
                    ranges: ranges.clone(),
                });
            }
        }
        match mode {
            crate::EditMode::Direct => {
                if matches!(location.owner, TargetOwner::Block(_)) || insertion_block.is_some() {
                    commands.extend(Self::replace_commands(&insertion, &canonical, text));
                } else if !canonical.is_empty() {
                    commands.push(Command::DeleteText { ranges: canonical });
                }
            }
            crate::EditMode::Suggest { id, author } => {
                if !canonical.is_empty() || insertion_proposal.is_none() && !text.is_empty() {
                    let (block, insertion) = match &location.owner {
                        TargetOwner::Block(block) => (block.clone(), insertion.clone()),
                        TargetOwner::Proposal(_) => {
                            if let Some(block) = &insertion_block {
                                (block.clone(), insertion.clone())
                            } else {
                                let first = parts
                                    .iter()
                                    .find_map(|part| match &part.owner {
                                        TargetOwner::Block(block) => Some((block, part.start)),
                                        TargetOwner::Proposal(_) => None,
                                    })
                                    .ok_or_else(|| {
                                        DocumentError::Invalid(
                                            "Missing canonical replacement target".into(),
                                        )
                                    })?;
                                (first.0.clone(), self.point(first.0, first.1)?)
                            }
                        }
                    };
                    commands.push(Command::ProposeReplacement {
                        id: id.clone(),
                        author: author.clone(),
                        block,
                        at: insertion,
                        ranges: canonical,
                        text: if insertion_proposal.is_none() {
                            text.into()
                        } else {
                            String::new()
                        },
                    });
                }
            }
            crate::EditMode::Continue { .. } => unreachable!(),
        }
        for (proposal, ranges) in proposed {
            if rejected.contains(&proposal) {
                continue;
            }
            let view = self
                .view()?
                .proposals
                .into_iter()
                .find(|view| view.proposal.id == proposal)
                .ok_or_else(|| DocumentError::NotFound {
                    kind: "proposal",
                    id: proposal.clone(),
                })?;
            let removed: usize = ranges
                .iter()
                .map(|range| self.range_offsets(range).map(|(start, end)| end - start))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .sum();
            let replacement = insertion_proposal.as_ref() == Some(&proposal) && !text.is_empty();
            if removed == crate::utf16_len(&view.text)
                && !replacement
                && matches!(
                    view.proposal.action,
                    crate::ProposalAction::Text {
                        ref delete_target,
                        ..
                    } if delete_target.is_empty()
                )
            {
                commands.push(Command::RejectProposal { id: proposal });
            }
        }
        Ok(commands)
    }

    pub(crate) fn split_container_commands(
        &self,
        id: &str,
        at: usize,
        new_id: &str,
    ) -> Result<Vec<Command>> {
        let block = self.active_block(id)?;
        self.require_container(&block.kind)?;
        let children: Vec<_> = self
            .blocks()?
            .into_iter()
            .filter(|b| b.parent.as_deref() == Some(id))
            .collect();
        if at > children.len() {
            return Err(DocumentError::Invalid(
                "Container split is out of bounds".into(),
            ));
        }
        let mut commands = vec![Command::InsertBlock {
            block: SeedBlock {
                id: new_id.into(),
                kind: block.kind,
                attrs: block.attrs,
                parent: block.parent,
                position: block.position + 1,
                text: String::new(),
            },
        }];
        commands.extend(children[at..].iter().map(|child| Command::MoveBlock {
            block: child.id.clone(),
            parent: Some(new_id.into()),
            before: None,
        }));
        Ok(commands)
    }

    pub(crate) fn join_container_commands(&self, left: &str, right: &str) -> Result<Vec<Command>> {
        let a = self.active_block(left)?;
        let b = self.active_block(right)?;
        self.require_container(&a.kind)?;
        self.require_container(&b.kind)?;
        let blocks = self.blocks()?;
        let siblings: Vec<_> = blocks.iter().filter(|b| b.parent == a.parent).collect();
        if !siblings
            .windows(2)
            .any(|pair| pair[0].id == left && pair[1].id == right)
        {
            return Err(DocumentError::Conflict(
                "Join requires adjacent sibling blocks".into(),
            ));
        }
        let mut commands: Vec<_> = blocks
            .iter()
            .filter(|b| b.parent.as_deref() == Some(right))
            .map(|b| Command::MoveBlock {
                block: b.id.clone(),
                parent: Some(left.into()),
                before: None,
            })
            .collect();
        commands.push(Command::DeleteBlock {
            block: right.into(),
        });
        Ok(commands)
    }

    fn require_container(&self, kind: &str) -> Result<()> {
        if crate::block_capabilities(kind)
            .is_some_and(|c| c.content == BlockContentModel::Container)
        {
            Ok(())
        } else {
            Err(DocumentError::Invalid(
                "This action requires a container block".into(),
            ))
        }
    }

    /// Move ownership of existing characters. Neither temporary blocks nor copied
    /// text participate, so review targets and concurrent source edits survive.
    pub fn move_text(
        &mut self,
        block: &str,
        proposal_id: Option<&str>,
        start: &TextPoint,
        end: &TextPoint,
        to: &TextPoint,
    ) -> Result<()> {
        self.atomic(|d| {
            let origin = d.locate_point(start)?.ok_or_else(|| {
                DocumentError::Conflict("Text move start is no longer visible".into())
            })?;
            let finish = d.locate_point(end)?.ok_or_else(|| {
                DocumentError::Conflict("Text move end is no longer visible".into())
            })?;
            let destination = d.locate_point(to)?.ok_or_else(|| {
                DocumentError::Conflict("Text move destination is no longer visible".into())
            })?;
            let mut proposal = proposal_id.map(|id| d.proposal(id)).transpose()?;
            let mut source;
            let mut target;
            let (origin_offset, finish_offset, destination_offset);
            if let Some(proposal) = &proposal {
                if proposal.state != crate::ProposalState::Open {
                    return Err(DocumentError::Conflict("Proposal already decided".into()));
                }
                let crate::ProposalAction::InsertBlocks { blocks, .. } = &proposal.action else {
                    return Err(DocumentError::Invalid(
                        "Proposal does not contain blocks".into(),
                    ));
                };
                let owner = TargetOwner::Proposal(proposal.id.clone());
                if origin.owner != owner || finish.owner != owner || destination.owner != owner {
                    return Err(DocumentError::Invalid(
                        "Move text within one proposal".into(),
                    ));
                }
                let a = origin.proposed_block.as_ref().ok_or_else(|| {
                    DocumentError::Invalid("Move requires a proposed block".into())
                })?;
                let b = finish.proposed_block.as_ref().ok_or_else(|| {
                    DocumentError::Invalid("Move requires a proposed block".into())
                })?;
                let c = destination.proposed_block.as_ref().ok_or_else(|| {
                    DocumentError::Invalid("Move requires a proposed block".into())
                })?;
                if a.block != block || b.block != block {
                    return Err(DocumentError::Invalid(
                        "Move requires an ordered range in one block".into(),
                    ));
                }
                let lookup = |id: &str| {
                    blocks.iter().find(|b| b.id == id).cloned().ok_or_else(|| {
                        DocumentError::Conflict("Proposed text owner is missing".into())
                    })
                };
                source = lookup(block)?;
                target = lookup(&c.block)?;
                (origin_offset, finish_offset, destination_offset) = (a.offset, b.offset, c.offset);
            } else {
                if origin.owner != TargetOwner::Block(block.into()) || finish.owner != origin.owner
                {
                    return Err(DocumentError::Invalid(
                        "Text move requires an ordered range in one block".into(),
                    ));
                }
                let TargetOwner::Block(destination_id) = &destination.owner else {
                    return Err(DocumentError::Invalid(
                        "Move text within document blocks".into(),
                    ));
                };
                source = d.active_block(block)?;
                target = d.active_block(destination_id)?;
                (origin_offset, finish_offset, destination_offset) =
                    (origin.offset, finish.offset, destination.offset);
            }
            if finish_offset < origin_offset {
                return Err(DocumentError::Invalid(
                    "Text move requires an ordered range".into(),
                ));
            }
            if finish_offset == origin_offset
                || target.id == source.id
                    && (origin_offset..=finish_offset).contains(&destination_offset)
            {
                return Ok(());
            }
            if !crate::carries_inline_content(&source.kind)
                || !crate::carries_inline_content(&target.kind)
            {
                return Err(DocumentError::Invalid(
                    "Text move requires text blocks".into(),
                ));
            }
            let (prefix, remainder) = d.partition_segments(&source.segments, start)?;
            let (moved, suffix) = d.partition_segments(&remainder, end)?;
            source.segments = prefix.into_iter().chain(suffix).collect();
            if target.id == source.id {
                target = source.clone();
            }
            let (prefix, suffix) = d.partition_segments(&target.segments, to)?;
            target.segments = prefix.into_iter().chain(moved).chain(suffix).collect();
            if let Some(proposal) = &mut proposal {
                let crate::ProposalAction::InsertBlocks { blocks, .. } = &mut proposal.action
                else {
                    unreachable!()
                };
                for block in blocks.iter_mut() {
                    if block.id == source.id {
                        *block = source.clone();
                    }
                    if block.id == target.id {
                        *block = target.clone();
                    }
                }
                proposal.segments = blocks.iter().flat_map(|b| b.segments.clone()).collect();
                d.update_review_time(&mut proposal.metadata);
                d.put_record(crate::PROPOSALS, &proposal.id, proposal)
            } else {
                d.put_record(BLOCKS, &source.id, &source)?;
                d.put_record(BLOCKS, &target.id, &target)
            }
        })
    }
}

impl Document {
    pub(crate) fn replace_selection_commands(
        &self,
        mode: &crate::EditMode,
        anchor: &TextPoint,
        focus: &TextPoint,
        text: &str,
    ) -> Result<Vec<Command>> {
        let missing = || DocumentError::Conflict("Selection is no longer visible".into());
        let anchor_location = self.locate_point(anchor)?.ok_or_else(missing)?;
        let focus_location = self.locate_point(focus)?.ok_or_else(missing)?;
        let (views, proposal) = match (&anchor_location.owner, &focus_location.owner) {
            (TargetOwner::Block(_), TargetOwner::Block(_)) => (self.view()?.blocks, None),
            (TargetOwner::Proposal(a), TargetOwner::Proposal(b)) if a == b => {
                if anchor_location.proposed_block.is_none()
                    && focus_location.proposed_block.is_none()
                {
                    let (at, start, end) = if anchor_location.offset <= focus_location.offset {
                        (anchor, anchor_location.offset, focus_location.offset)
                    } else {
                        (focus, focus_location.offset, anchor_location.offset)
                    };
                    return self.edit_commands(
                        mode,
                        &crate::EditAction::ReplaceText {
                            at: at.clone(),
                            ranges: self.proposal_selection(a, start, end)?,
                            text: text.into(),
                        },
                    );
                }
                (self.proposed_blocks_view(a)?, Some(a.clone()))
            }
            _ => return Err(DocumentError::Invalid(
                "Replace a selection within one text owner; decide intervening suggestions first"
                    .into(),
            )),
        };
        let address = |location: &crate::ResolvedPoint| -> Result<(usize, usize)> {
            let (id, offset) = match &location.owner {
                TargetOwner::Block(id) => (id, location.offset),
                TargetOwner::Proposal(_) => {
                    let point = location.proposed_block.as_ref().ok_or_else(missing)?;
                    (&point.block, point.offset)
                }
            };
            Ok((
                views
                    .iter()
                    .position(|view| &view.block.id == id)
                    .ok_or_else(missing)?,
                offset,
            ))
        };
        let a = address(&anchor_location)?;
        let b = address(&focus_location)?;
        let (start, end, at) = if a <= b {
            (a, b, anchor)
        } else {
            (b, a, focus)
        };
        let selected = &views[start.0..=end.0];
        let joins = selected.len() > 1
            && (matches!(mode, crate::EditMode::Direct) || proposal.is_some())
            && selected.iter().all(|v| {
                v.block.parent == selected[0].block.parent
                    && crate::carries_inline_content(&v.block.kind)
            });
        let mut ranges = Vec::new();
        for (index, view) in selected.iter().enumerate() {
            // Whole middle blocks are hidden structurally. Their text remains
            // available for review and undo, with its original identities.
            if joins && index > 0 && index + 1 < selected.len() {
                continue;
            }
            if !crate::carries_inline_content(&view.block.kind) {
                continue;
            }
            let from = if index == 0 { start.1 } else { 0 };
            let to = if index + 1 == selected.len() {
                end.1
            } else {
                view.text.encode_utf16().count()
            };
            ranges.extend(self.selection_in_segments(&view.block.segments, from, to)?);
        }
        let mut commands = self.edit_commands(
            mode,
            &crate::EditAction::ReplaceText {
                at: at.clone(),
                ranges,
                text: text.into(),
            },
        )?;
        // Canonical suggestions retain the selected text until the review
        // decision. Editing an existing proposal changes its own block tree.
        if !matches!(mode, crate::EditMode::Direct) && proposal.is_none() {
            return Ok(commands);
        }
        let first = &selected[0].block;
        let last = &selected[selected.len() - 1].block;
        if !joins {
            return Ok(commands);
        }
        if let Some(proposal) = proposal {
            let removed: std::collections::BTreeSet<_> = selected[1..selected.len() - 1]
                .iter()
                .map(|v| &v.block.id)
                .collect();
            if !removed.is_empty() {
                commands.push(Command::SetProposedStructure {
                    proposal: proposal.clone(),
                    blocks: views
                        .iter()
                        .filter(|v| !removed.contains(&v.block.id))
                        .map(|v| crate::ProposedBlockPlacement::Existing {
                            block: v.block.id.clone(),
                            parent: v.block.parent.clone(),
                            position: v.block.position,
                        })
                        .collect(),
                });
            }
            commands.push(Command::JoinProposedBlocks {
                proposal,
                left: first.id.clone(),
                right: last.id.clone(),
            });
        } else {
            commands.extend(
                selected[1..selected.len() - 1]
                    .iter()
                    .map(|v| Command::DeleteBlock {
                        block: v.block.id.clone(),
                    }),
            );
            commands.push(Command::JoinBlocks {
                left: first.id.clone(),
                right: last.id.clone(),
            });
        }
        Ok(commands)
    }
}
