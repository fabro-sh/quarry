use crate::{
    BLOCKS, Block, COMMENTS, Comment, DiscussionState, Document, DocumentError, PROPOSALS,
    Proposal, ProposalAction, ProposalState, ResolvedTarget, Result, ReviewMetadata, SeedBlock,
    TargetFragment, TargetOwner, TargetState, TextPoint, TextRange, text_slice,
};
use automerge::ReadDoc;
use std::collections::{BTreeMap, BTreeSet};

impl Document {
    pub(crate) fn new_review_metadata(&self) -> ReviewMetadata {
        let time = self.command_time.clone().unwrap_or_default();
        ReviewMetadata {
            created_at: time.clone(),
            updated_at: time,
            unattached_reason: None,
            legacy_record: None,
        }
    }

    pub(crate) fn update_review_time(&self, metadata: &mut ReviewMetadata) {
        if let Some(time) = self.command_time.as_ref().filter(|t| !t.is_empty()) {
            metadata.updated_at.clone_from(time);
        }
    }
    pub fn comments(&self) -> Result<Vec<Comment>> {
        Ok(self
            .records::<Comment>(COMMENTS)?
            .into_iter()
            .filter(|c| !c.deleted)
            .collect())
    }

    pub fn comment(&self, id: &str) -> Result<Comment> {
        self.record(COMMENTS, id)
    }

    /// Capture immutable character identities for a private review draft.
    pub fn capture_target(&self, ranges: &[TextRange]) -> Result<(Vec<TargetFragment>, String)> {
        let mut target = Vec::new();
        let mut quote = String::new();
        for range in ranges {
            let (start, end) = self.range_offsets(range)?;
            if start == end {
                return Err(DocumentError::Conflict("Review target was deleted".into()));
            }
            for segment in self.source_segments(&range.source)?.iter() {
                for run in &segment.runs {
                    let from = start.max(run.source_start);
                    let to = end.min(run.source_start + crate::utf16_len(&run.text));
                    if from < to {
                        quote.push_str(text_slice(
                            &run.text,
                            from - run.source_start,
                            to - run.source_start,
                        )?);
                    }
                }
            }
            let text = self.crdt.text(self.source(&range.source)?)?;
            let selected = text_slice(&text, start, end)?;
            let last_width = selected
                .chars()
                .next_back()
                .ok_or_else(|| DocumentError::Invalid("Empty target".into()))?
                .len_utf16();
            target.push(TargetFragment {
                source: range.source.clone(),
                first: self.point_at(&range.source, start)?.cursor,
                last: self.point_at(&range.source, end - last_width)?.cursor,
                last_width,
            });
        }
        if !ranges.is_empty() && quote.is_empty() {
            return Err(DocumentError::Invalid(
                "Review target contains no text".into(),
            ));
        }
        Ok((target, quote))
    }

    pub fn add_comment(
        &mut self,
        id: &str,
        author: &str,
        body: &str,
        ranges: &[TextRange],
    ) -> Result<()> {
        self.atomic(|d| {
            d.require_new_review(id)?;
            if ranges.is_empty() {
                return Err(DocumentError::Invalid(
                    "Comment requires a text target".into(),
                ));
            }
            let (target, original_quote) = d.capture_target(ranges)?;
            d.put_record(
                COMMENTS,
                id,
                &Comment {
                    id: id.into(),
                    author: author.into(),
                    body: body.into(),
                    target,
                    original_quote,
                    state: DiscussionState::Open,
                    parent_id: None,
                    deleted: false,
                    metadata: d.new_review_metadata(),
                },
            )
        })
    }

    pub fn reply_comment(
        &mut self,
        id: &str,
        parent: &str,
        author: &str,
        body: &str,
    ) -> Result<()> {
        self.atomic(|d| {
            d.require_new_review(id)?;
            let parent = match d.comment(parent) {
                Ok(comment) => {
                    if comment.deleted {
                        return Err(DocumentError::Conflict("Comment was deleted".into()));
                    }
                    comment.parent_id.unwrap_or_else(|| parent.into())
                }
                Err(DocumentError::NotFound { .. }) => parent.into(),
                Err(error) => return Err(error),
            };
            let target = d.review_target(&parent)?;
            d.put_record(
                COMMENTS,
                id,
                &Comment {
                    id: id.into(),
                    author: author.into(),
                    body: body.into(),
                    target: Vec::new(),
                    original_quote: target
                        .attachments
                        .iter()
                        .map(|a| a.quote.as_str())
                        .collect(),
                    state: DiscussionState::Open,
                    parent_id: Some(parent),
                    deleted: false,
                    metadata: d.new_review_metadata(),
                },
            )
        })
    }

    pub fn edit_comment(&mut self, id: &str, body: &str) -> Result<()> {
        self.atomic(|d| {
            let mut comment = d.comment(id)?;
            if comment.deleted {
                return Err(DocumentError::Conflict("Comment was deleted".into()));
            }
            comment.body = body.into();
            d.update_review_time(&mut comment.metadata);
            d.put_record(COMMENTS, id, &comment)
        })
    }

    pub fn delete_comment(&mut self, id: &str) -> Result<()> {
        self.atomic(|d| {
            d.comment(id)?;
            for mut comment in d.comments()? {
                if comment.id == id || comment.parent_id.as_deref() == Some(id) {
                    comment.deleted = true;
                    comment.body.clear();
                    d.update_review_time(&mut comment.metadata);
                    d.put_record(COMMENTS, &comment.id, &comment)?;
                }
            }
            Ok(())
        })
    }

    pub fn resolve_comment(&mut self, id: &str, resolved: bool) -> Result<()> {
        self.atomic(|d| {
            let mut comment = d.comment(id)?;
            if comment.deleted || comment.parent_id.is_some() {
                return Err(DocumentError::Conflict(
                    "Decision requires an existing root comment".into(),
                ));
            }
            comment.state = if resolved {
                DiscussionState::Resolved
            } else {
                DiscussionState::Open
            };
            d.update_review_time(&mut comment.metadata);
            d.put_record(COMMENTS, id, &comment)
        })
    }

    /// Import must explicitly record an unknown target, never guess by quote.
    pub fn import_unattached_comment(&mut self, comment: Comment) -> Result<()> {
        self.atomic(|d| {
            d.require_new_review(&comment.id)?;
            if !comment.target.is_empty() || comment.metadata.unattached_reason.is_none() {
                return Err(DocumentError::Invalid(
                    "Unattached import requires a reason and no native target".into(),
                ));
            }
            d.put_record(COMMENTS, &comment.id, &comment)
        })
    }

    pub fn set_comment_metadata(&mut self, id: &str, metadata: ReviewMetadata) -> Result<()> {
        self.atomic(|d| {
            let mut comment = d.comment(id)?;
            comment.metadata = metadata;
            d.put_record(COMMENTS, id, &comment)
        })
    }

    pub fn comment_target(&self, id: &str) -> Result<ResolvedTarget> {
        let comment = self.comment(id)?;
        if let Some(parent) = comment.parent_id {
            return self.review_target(&parent);
        }
        if comment.target.is_empty() && comment.metadata.unattached_reason.is_some() {
            return Ok(ResolvedTarget {
                state: TargetState::Unattached,
                attachments: Vec::new(),
            });
        }
        self.resolve_target(&comment.target)
    }

    fn review_target(&self, id: &str) -> Result<ResolvedTarget> {
        if self.comment(id).is_ok() {
            return self.comment_target(id);
        }
        if self.proposal(id).is_ok() {
            return self.proposal_target(id);
        }
        let conflict = self.conflict(id)?;
        if let Some(block) = conflict
            .after
            .and_then(|id| self.block(&id).ok())
            .filter(|b| !b.deleted)
        {
            Ok(ResolvedTarget {
                state: TargetState::Attached,
                attachments: vec![crate::Attachment {
                    owner: TargetOwner::Block(block.id),
                    start: 0,
                    end: 0,
                    quote: String::new(),
                }],
            })
        } else {
            Ok(ResolvedTarget {
                state: TargetState::Unattached,
                attachments: Vec::new(),
            })
        }
    }

    pub(crate) fn target_offsets(&self, target: &TargetFragment) -> Result<(usize, usize)> {
        let start = self.resolve_point(&TextPoint {
            source: target.source.clone(),
            cursor: target.first.clone(),
        })?;
        let last = self.resolve_point(&TextPoint {
            source: target.source.clone(),
            cursor: target.last.clone(),
        })?;
        if !(1..=2).contains(&target.last_width) || start > last {
            return Err(DocumentError::Invalid(
                "Invalid target character identities".into(),
            ));
        }
        let end = last
            .checked_add(target.last_width)
            .ok_or_else(|| DocumentError::Invalid("Target offset overflow".into()))?;
        let source = self.crdt.text(self.source(&target.source)?)?;
        if text_slice(&source, last, end)?.chars().count() != 1 {
            return Err(DocumentError::Invalid(
                "Target end is not one character".into(),
            ));
        }
        Ok((start, end))
    }

    pub fn resolve_target(&self, target: &[TargetFragment]) -> Result<ResolvedTarget> {
        let mut attachments = Vec::new();
        let mut surviving = false;
        let blocks = self.cached_blocks()?;
        let proposals = self.proposals()?;
        for fragment in target {
            let (start, end) = self.target_offsets(fragment)?;
            for segment in self.source_segments(&fragment.source)?.iter() {
                if segment.runs.iter().any(|r| {
                    start.max(r.source_start) < end.min(r.source_start + crate::utf16_len(&r.text))
                }) {
                    surviving = true;
                }
            }
            for block in &blocks.ordered {
                if !block
                    .segments
                    .iter()
                    .any(|reference| reference.source == fragment.source)
                {
                    continue;
                }
                attachments.extend(self.attach_range(
                    TargetOwner::Block(block.id.clone()),
                    &block.segments,
                    &fragment.source,
                    start,
                    end,
                )?);
            }
            for proposal in proposals.iter().filter(|p| p.state == ProposalState::Open) {
                if !proposal
                    .segments
                    .iter()
                    .any(|reference| reference.source == fragment.source)
                {
                    continue;
                }
                attachments.extend(self.attach_range(
                    TargetOwner::Proposal(proposal.id.clone()),
                    &proposal.segments,
                    &fragment.source,
                    start,
                    end,
                )?);
            }
        }

        let state = if attachments.is_empty() {
            if surviving {
                TargetState::Hidden
            } else {
                TargetState::Deleted
            }
        } else {
            TargetState::Attached
        };
        let mut joined: Vec<crate::Attachment> = Vec::new();
        for attachment in attachments {
            if let Some(last) = joined
                .last_mut()
                .filter(|last| last.owner == attachment.owner && last.end == attachment.start)
            {
                last.end = attachment.end;
                last.quote.push_str(&attachment.quote);
            } else {
                joined.push(attachment);
            }
        }
        Ok(ResolvedTarget {
            state,
            attachments: joined,
        })
    }

    pub fn proposal(&self, id: &str) -> Result<Proposal> {
        self.record(PROPOSALS, id)
    }
    pub fn proposals(&self) -> Result<Vec<Proposal>> {
        self.records(PROPOSALS)
    }

    pub fn import_unavailable_proposal(&mut self, proposal: Proposal) -> Result<()> {
        self.atomic(|d| {
            d.require_new_review(&proposal.id)?;
            if !matches!(proposal.action, ProposalAction::Unavailable { .. })
                || !proposal.segments.is_empty()
            {
                return Err(DocumentError::Invalid(
                    "Unavailable import requires a reason and no native target".into(),
                ));
            }
            d.put_record(PROPOSALS, &proposal.id, &proposal)
        })
    }

    /// Older stores recorded a decision without distinguishing acceptance
    /// from rejection. Preserve that fact without applying a change again.
    pub fn close_imported_proposal(&mut self, id: &str) -> Result<()> {
        self.atomic(|d| {
            let mut proposal = d.proposal(id)?;
            if proposal.state == ProposalState::Open {
                proposal.state = ProposalState::Closed;
                d.put_record(PROPOSALS, id, &proposal)?;
            }
            Ok(())
        })
    }

    pub fn conflicts(&self) -> Result<Vec<crate::Conflict>> {
        self.records(crate::CONFLICTS)
    }
    pub fn conflict(&self, id: &str) -> Result<crate::Conflict> {
        self.record(crate::CONFLICTS, id)
    }
    pub fn add_conflict(&mut self, conflict: crate::Conflict) -> Result<()> {
        self.atomic(|d| {
            d.require_new_review(&conflict.id)?;
            d.put_record(crate::CONFLICTS, &conflict.id, &conflict)
        })
    }
    pub fn resolve_conflict(&mut self, id: &str) -> Result<()> {
        self.atomic(|d| {
            let mut conflict = d.conflict(id)?;
            if conflict.resolved {
                return Err(DocumentError::Conflict("Conflict already decided".into()));
            }
            conflict.resolved = true;
            d.update_review_time(&mut conflict.metadata);
            d.put_record(crate::CONFLICTS, id, &conflict)
        })
    }

    pub fn propose_insertion(
        &mut self,
        id: &str,
        author: &str,
        block: &str,
        at: &TextPoint,
        text: &str,
    ) -> Result<()> {
        self.propose_replacement(id, author, block, at, &[], text)
    }

    pub fn propose_replacement(
        &mut self,
        id: &str,
        author: &str,
        block: &str,
        at: &TextPoint,
        ranges: &[TextRange],
        text: &str,
    ) -> Result<()> {
        self.atomic(|d| {
            d.require_new_review(id)?;
            let owner = d
                .locate_point(at)?
                .ok_or_else(|| DocumentError::Conflict("Proposal position was deleted".into()))?;
            if owner.owner != TargetOwner::Block(block.into()) {
                return Err(DocumentError::Conflict(
                    "Proposal position does not belong to the block".into(),
                ));
            }
            let (delete_target, original_quote) = d.capture_target(ranges)?;
            if !ranges.is_empty() {
                d.validate_replacement_target(&delete_target, &original_quote)?;
            }
            let segments = vec![d.create_source(text)?];
            d.put_record(
                PROPOSALS,
                id,
                &Proposal {
                    id: id.into(),
                    author: author.into(),
                    body: String::new(),
                    action: ProposalAction::Text {
                        at: at.clone(),
                        delete_target,
                        original_quote,
                    },
                    segments,
                    state: ProposalState::Open,
                    metadata: d.new_review_metadata(),
                },
            )
        })
    }

    /// Compose only the proposal explicitly named by the caller. Canonical
    /// additions must touch its current target. Proposed characters stay in
    /// their original sources, preserving comments and collaborator edits.
    pub fn continue_text_proposal(
        &mut self,
        id: &str,
        author: &str,
        at: &TextPoint,
        ranges: &[TextRange],
        text: &str,
    ) -> Result<()> {
        self.atomic(|d| {
            let mut proposal = d.proposal(id)?;
            if proposal.state != ProposalState::Open || proposal.author != author {
                return Err(DocumentError::Conflict(
                    "Continue an open suggestion by the same author".into(),
                ));
            }
            let ProposalAction::Text {
                at: original_at,
                delete_target,
                original_quote,
            } = &proposal.action
            else {
                return Err(DocumentError::Invalid(
                    "This suggestion is not a text edit".into(),
                ));
            };
            if ranges.is_empty() && text.is_empty() {
                return Err(DocumentError::Invalid(
                    "A continuation requires an edit".into(),
                ));
            }
            if !delete_target.is_empty() {
                d.validate_replacement_target(delete_target, original_quote)?;
            }
            let position = d
                .locate_point(at)?
                .ok_or_else(|| DocumentError::Conflict("Edit position was deleted".into()))?;
            let original_position = d
                .locate_point(original_at)?
                .ok_or_else(|| DocumentError::Conflict("Suggestion position was deleted".into()))?;
            if position.owner != original_position.owner
                || !matches!(position.owner, TargetOwner::Block(_))
            {
                return Err(DocumentError::Conflict(
                    "Continue within the suggestion's canonical block".into(),
                ));
            }
            let mut parts = d.resolve_target(delete_target)?.attachments;
            if parts.is_empty() {
                parts.push(crate::Attachment {
                    owner: original_position.owner.clone(),
                    start: original_position.offset,
                    end: original_position.offset,
                    quote: String::new(),
                });
            }
            if ranges.is_empty() {
                if position.offset != parts[0].start
                    && (position.owner != parts[parts.len() - 1].owner
                        || position.offset != parts[parts.len() - 1].end)
                {
                    return Err(DocumentError::Conflict(
                        "Continuation does not touch the suggestion".into(),
                    ));
                }
                // Typing after a selection deletion also works when that
                // selection crossed blocks. No deletion target is recaptured.
                d.update_review_time(&mut proposal.metadata);
                d.put_record(PROPOSALS, id, &proposal)?;
                return d.append_proposal_text(&proposal, text);
            }
            let (added, quote) = d.capture_target(ranges)?;
            d.validate_replacement_target(&added, &quote)?;
            let addition = d.resolve_target(&added)?.attachments;
            if addition.is_empty() || position.offset != addition[0].start {
                return Err(DocumentError::Conflict(
                    "Continuation position must start its added range".into(),
                ));
            }
            parts.extend(addition);
            parts.sort_by_key(|part| (part.start, part.end));
            if parts.iter().any(|part| part.owner != position.owner)
                || parts.windows(2).any(|pair| pair[0].end != pair[1].start)
            {
                return Err(DocumentError::Conflict(
                    "Continuation ranges must be adjacent and must not overlap".into(),
                ));
            }
            let TargetOwner::Block(block) = &position.owner else {
                unreachable!()
            };
            let start = parts[0].start;
            let end = parts[parts.len() - 1].end;
            let (delete_target, original_quote) =
                d.capture_target(&d.selection(block, start, end)?)?;
            proposal.action = ProposalAction::Text {
                at: d.point(block, start)?,
                delete_target,
                original_quote,
            };
            d.update_review_time(&mut proposal.metadata);
            d.put_record(PROPOSALS, id, &proposal)?;
            d.append_proposal_text(&proposal, text)
        })
    }

    fn append_proposal_text(&mut self, proposal: &Proposal, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let length = proposal.segments.iter().try_fold(0, |length, reference| {
            Ok::<_, DocumentError>(
                length
                    + self
                        .segment(reference)?
                        .runs
                        .iter()
                        .map(|run| crate::utf16_len(&run.text))
                        .sum::<usize>(),
            )
        })?;
        self.insert_text(&self.proposal_point(&proposal.id, length)?, text)
    }

    pub(crate) fn validate_replacement_target(
        &self,
        target: &[TargetFragment],
        original_quote: &str,
    ) -> Result<()> {
        let resolved = self.resolve_target(target)?;
        // Compare characters in their captured source order. Splitting or
        // moving a block changes its presentation order, not this decision.
        let mut quote = String::new();
        for fragment in target {
            let (start, end) = self.target_offsets(fragment)?;
            for segment in self.source_segments(&fragment.source)?.iter() {
                for run in &segment.runs {
                    let from = start.max(run.source_start);
                    let to = end.min(run.source_start + crate::utf16_len(&run.text));
                    if from < to {
                        quote.push_str(text_slice(
                            &run.text,
                            from - run.source_start,
                            to - run.source_start,
                        )?);
                    }
                }
            }
        }
        let visible: usize = resolved
            .attachments
            .iter()
            .map(|part| crate::utf16_len(&part.quote))
            .sum();
        if quote != original_quote
            || visible != crate::utf16_len(&quote)
            || resolved
                .attachments
                .iter()
                .any(|part| !matches!(part.owner, TargetOwner::Block(_)))
        {
            return Err(DocumentError::Conflict(
                "Proposed replacement target changed; create a new suggestion for the current text"
                    .into(),
            ));
        }
        Ok(())
    }

    /// A formatting decision keeps the selected native characters. Accepting
    /// it never replaces text or transfers comments to copied characters.
    pub fn propose_format(
        &mut self,
        id: &str,
        author: &str,
        ranges: &[TextRange],
        name: &str,
        value: &serde_json::Value,
    ) -> Result<()> {
        crate::schema::validate_format(name, value)?;
        self.atomic(|d| {
            d.require_new_review(id)?;
            if ranges.is_empty() {
                return Err(DocumentError::Invalid(
                    "Formatting requires a text target".into(),
                ));
            }
            let (target, _) = d.capture_target(ranges)?;
            let expected = d.format_snapshot(&target, name)?;
            d.put_record(
                PROPOSALS,
                id,
                &Proposal {
                    id: id.into(),
                    author: author.into(),
                    body: String::new(),
                    action: ProposalAction::Format {
                        target,
                        name: name.into(),
                        value: value.clone(),
                        expected,
                    },
                    segments: Vec::new(),
                    state: ProposalState::Open,
                    metadata: d.new_review_metadata(),
                },
            )
        })
    }

    fn format_snapshot(
        &self,
        target: &[TargetFragment],
        name: &str,
    ) -> Result<Vec<(String, serde_json::Value)>> {
        let resolved = self.resolve_target(target)?;
        if resolved.state != TargetState::Attached {
            return Err(DocumentError::Conflict(
                "Formatting target is no longer visible".into(),
            ));
        }
        let mut result: Vec<(String, serde_json::Value)> = Vec::new();
        for fragment in target {
            let (start, end) = self.target_offsets(fragment)?;
            for segment in self.source_segments(&fragment.source)?.iter() {
                for run in &segment.runs {
                    let from = start.max(run.source_start);
                    let to = end.min(run.source_start + crate::utf16_len(&run.text));
                    if from >= to {
                        continue;
                    }
                    let text =
                        text_slice(&run.text, from - run.source_start, to - run.source_start)?;
                    let value = run
                        .marks
                        .get(name)
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    if let Some((previous, mark)) = result.last_mut()
                        && *mark == value
                    {
                        previous.push_str(text);
                    } else {
                        result.push((text.into(), value));
                    }
                }
            }
        }
        let visible: usize = resolved
            .attachments
            .iter()
            .map(|part| crate::utf16_len(&part.quote))
            .sum();
        let captured: usize = result.iter().map(|(text, _)| crate::utf16_len(text)).sum();
        if visible != captured {
            return Err(DocumentError::Conflict(
                "Formatting target is partly hidden".into(),
            ));
        }
        Ok(result)
    }

    /// Block properties are a review decision. Text stays in its existing
    /// native segments, including comments and later text edits.
    pub fn propose_block_update(
        &mut self,
        id: &str,
        author: &str,
        block: &str,
        kind: &str,
        attrs: BTreeMap<String, serde_json::Value>,
    ) -> Result<()> {
        self.atomic(|d| {
            d.require_new_review(id)?;
            let previous = d.active_block(block)?;
            let attrs = crate::schema::normalize_attrs(kind, attrs)?;
            let mut candidate = d.fork();
            candidate.set_block(block, kind, attrs.clone())?;
            d.put_record(
                PROPOSALS,
                id,
                &Proposal {
                    id: id.into(),
                    author: author.into(),
                    body: String::new(),
                    segments: Vec::new(),
                    state: ProposalState::Open,
                    metadata: d.new_review_metadata(),
                    action: ProposalAction::UpdateBlock {
                        block: block.into(),
                        block_kind: kind.into(),
                        attrs,
                        expected_kind: previous.kind,
                        expected_attrs: previous.attrs,
                    },
                },
            )
        })
    }

    fn block_placement(&self, id: &str) -> Result<(Option<String>, Option<String>)> {
        let block = self.active_block(id)?;
        let siblings: Vec<_> = self
            .blocks()?
            .into_iter()
            .filter(|b| b.parent == block.parent)
            .collect();
        let before = siblings
            .iter()
            .find(|b| b.position > block.position)
            .map(|b| b.id.clone());
        Ok((block.parent, before))
    }

    /// A move changes placement only. Acceptance keeps the current text and
    /// all of its native review targets, including edits made after proposing.
    pub fn propose_block_move(
        &mut self,
        id: &str,
        author: &str,
        block: &str,
        parent: Option<String>,
        before: Option<String>,
    ) -> Result<()> {
        self.atomic(|d| {
            d.require_new_review(id)?;
            let (expected_parent, expected_before) = d.block_placement(block)?;
            d.validate_block_move(block, parent.as_deref(), before.as_deref())?;
            d.put_record(
                PROPOSALS,
                id,
                &Proposal {
                    id: id.into(),
                    author: author.into(),
                    body: String::new(),
                    segments: Vec::new(),
                    state: ProposalState::Open,
                    metadata: d.new_review_metadata(),
                    action: ProposalAction::MoveBlock {
                        block: block.into(),
                        parent,
                        before,
                        expected_parent,
                        expected_before,
                    },
                },
            )
        })
    }

    pub fn propose_block_delete(&mut self, id: &str, author: &str, block: &str) -> Result<()> {
        self.atomic(|d| {
            d.require_new_review(id)?;
            let expected = d.subtree_content(block)?;
            d.put_record(
                PROPOSALS,
                id,
                &Proposal {
                    id: id.into(),
                    author: author.into(),
                    body: String::new(),
                    action: ProposalAction::DeleteBlock {
                        block: block.into(),
                        expected,
                    },
                    segments: Vec::new(),
                    state: ProposalState::Open,
                    metadata: d.new_review_metadata(),
                },
            )
        })
    }

    pub fn propose_blocks(
        &mut self,
        id: &str,
        author: &str,
        parent: Option<String>,
        before: Option<String>,
        seeds: &[SeedBlock],
    ) -> Result<()> {
        self.atomic(|d| {
            d.require_new_review(id)?;
            if seeds.is_empty() {
                return Err(DocumentError::Invalid("Block proposal is empty".into()));
            }
            if let Some(parent) = &parent {
                d.active_block(parent)?;
            }
            if let Some(before) = &before
                && d.active_block(before)?.parent != parent
            {
                return Err(DocumentError::Invalid(
                    "Insertion destination is not a sibling".into(),
                ));
            }
            let mut blocks = Vec::new();
            let mut segments = Vec::new();
            let mut ids = BTreeSet::new();
            for seed in seeds {
                if seed.parent.is_none() {
                    let parent_kind = parent
                        .as_ref()
                        .map(|id| d.active_block(id).map(|b| b.kind))
                        .transpose()?;
                    crate::schema::validate_parent(&seed.kind, parent_kind.as_deref())?;
                }
                d.require_new(BLOCKS, &seed.id)?;
                if !ids.insert(seed.id.clone()) {
                    return Err(DocumentError::Invalid("Repeated proposed block ID".into()));
                }
                let reference = d.create_source(&seed.text)?;
                segments.push(reference.clone());
                blocks.push(Block {
                    id: seed.id.clone(),
                    kind: seed.kind.clone(),
                    attrs: crate::schema::normalize_attrs(&seed.kind, seed.attrs.clone())?,
                    parent: seed.parent.clone(),
                    position: seed.position,
                    segments: vec![reference],
                    deleted: false,
                });
            }
            if blocks
                .iter()
                .any(|b| b.parent.as_ref().is_some_and(|p| !ids.contains(p)))
            {
                return Err(DocumentError::Invalid(
                    "Proposed block parent is outside the proposal".into(),
                ));
            }
            let proposed: BTreeMap<_, _> = blocks.iter().map(|block| (&block.id, block)).collect();
            let mut placements = BTreeSet::new();
            for block in &blocks {
                if block.id.is_empty()
                    || block.kind.is_empty()
                    || !placements.insert((&block.parent, block.position))
                {
                    return Err(DocumentError::Invalid(
                        "Invalid proposed block identity or position".into(),
                    ));
                }
                let mut path = BTreeSet::from([&block.id]);
                let mut parent = block.parent.as_ref();
                while let Some(id) = parent {
                    if !path.insert(id) {
                        return Err(DocumentError::Invalid("Proposed block parent cycle".into()));
                    }
                    parent = proposed
                        .get(id)
                        .ok_or_else(|| DocumentError::Invalid("Missing proposed parent".into()))?
                        .parent
                        .as_ref();
                }
            }
            d.put_record(
                PROPOSALS,
                id,
                &Proposal {
                    id: id.into(),
                    author: author.into(),
                    body: String::new(),
                    action: ProposalAction::InsertBlocks {
                        parent,
                        before,
                        blocks,
                    },
                    segments,
                    state: ProposalState::Open,
                    metadata: d.new_review_metadata(),
                },
            )
        })
    }

    /// Edit the properties of an open proposed block in place. Its sources
    /// remain owned by the proposal until the reviewer accepts it.
    pub fn set_proposed_block(
        &mut self,
        proposal_id: &str,
        block_id: &str,
        kind: &str,
        attrs: BTreeMap<String, serde_json::Value>,
    ) -> Result<()> {
        self.atomic(|d| {
            let mut proposal = d.proposal(proposal_id)?;
            if proposal.state != ProposalState::Open {
                return Err(DocumentError::Conflict("Proposal already decided".into()));
            }
            let ProposalAction::InsertBlocks { blocks, .. } = &mut proposal.action else {
                return Err(DocumentError::Invalid(
                    "Proposal does not contain blocks".into(),
                ));
            };
            let block = blocks
                .iter_mut()
                .find(|block| block.id == block_id)
                .ok_or_else(|| DocumentError::NotFound {
                    kind: "proposed block",
                    id: block_id.into(),
                })?;
            if block.kind != kind && (block.kind == "raw_markdown" || kind == "raw_markdown") {
                return Err(DocumentError::Invalid(
                    "Replace a raw Markdown block explicitly to change its content model".into(),
                ));
            }
            block.kind = kind.into();
            block.attrs = crate::schema::normalize_attrs(kind, attrs)?;
            d.put_record(PROPOSALS, proposal_id, &proposal)
        })
    }

    /// Split proposed text without copying its characters or changing the
    /// canonical document. Both pieces keep the original native source.
    pub fn split_proposed_block(
        &mut self,
        proposal_id: &str,
        block_id: &str,
        at: &TextPoint,
        new_block_id: &str,
    ) -> Result<()> {
        self.atomic(|d| {
            d.require_new(BLOCKS, new_block_id)?;
            let mut proposal = d.proposal(proposal_id)?;
            if proposal.state != ProposalState::Open {
                return Err(DocumentError::Conflict("Proposal already decided".into()));
            }
            let ProposalAction::InsertBlocks { blocks, .. } = &mut proposal.action else {
                return Err(DocumentError::Invalid(
                    "Proposal does not contain blocks".into(),
                ));
            };
            if blocks.iter().any(|b| b.id == new_block_id) {
                return Err(DocumentError::Invalid(
                    "Proposed block identity already exists".into(),
                ));
            }
            let index = blocks
                .iter()
                .position(|b| b.id == block_id)
                .ok_or_else(|| DocumentError::NotFound {
                    kind: "proposed block",
                    id: block_id.into(),
                })?;
            let mut next = blocks[index].clone();
            if !crate::block_capabilities(&next.kind)
                .is_some_and(|c| c.content == crate::BlockContentModel::Text)
            {
                return Err(DocumentError::Invalid(
                    "Split requires a proposed text block".into(),
                ));
            }
            let (prefix, suffix) = d.partition_segments(&next.segments, at)?;
            blocks[index].segments = prefix;
            for sibling in blocks.iter_mut() {
                if sibling.parent == next.parent && sibling.position > next.position {
                    sibling.position += 1;
                }
            }
            next.id = new_block_id.into();
            next.position += 1;
            next.segments = suffix;
            blocks.insert(index + 1, next);
            proposal.segments = blocks.iter().flat_map(|b| b.segments.clone()).collect();
            d.update_review_time(&mut proposal.metadata);
            d.put_record(PROPOSALS, proposal_id, &proposal)
        })
    }

    pub fn join_proposed_blocks(
        &mut self,
        proposal_id: &str,
        left: &str,
        right: &str,
    ) -> Result<()> {
        self.atomic(|d| {
            let mut proposal = d.proposal(proposal_id)?;
            if proposal.state != ProposalState::Open {
                return Err(DocumentError::Conflict("Proposal already decided".into()));
            }
            let ProposalAction::InsertBlocks { blocks, .. } = &mut proposal.action else {
                return Err(DocumentError::Invalid(
                    "Proposal does not contain blocks".into(),
                ));
            };
            let left_index = blocks.iter().position(|b| b.id == left).ok_or_else(|| {
                DocumentError::NotFound {
                    kind: "proposed block",
                    id: left.into(),
                }
            })?;
            let right_index = blocks.iter().position(|b| b.id == right).ok_or_else(|| {
                DocumentError::NotFound {
                    kind: "proposed block",
                    id: right.into(),
                }
            })?;
            for index in [left_index, right_index] {
                if !crate::block_capabilities(&blocks[index].kind)
                    .is_some_and(|c| c.content == crate::BlockContentModel::Text)
                {
                    return Err(DocumentError::Invalid(
                        "Join requires proposed text blocks".into(),
                    ));
                }
            }
            let mut siblings: Vec<_> = blocks
                .iter()
                .filter(|b| b.parent == blocks[left_index].parent)
                .collect();
            siblings.sort_by_key(|b| b.position);
            if !siblings
                .windows(2)
                .any(|pair| pair[0].id == left && pair[1].id == right)
            {
                return Err(DocumentError::Conflict(
                    "Join requires adjacent proposed sibling blocks".into(),
                ));
            }
            let segments = blocks[right_index].segments.clone();
            blocks[left_index].segments.extend(segments);
            blocks.remove(right_index);
            proposal.segments = blocks.iter().flat_map(|b| b.segments.clone()).collect();
            d.update_review_time(&mut proposal.metadata);
            d.put_record(PROPOSALS, proposal_id, &proposal)
        })
    }

    /// Replace an open proposal's layout, retaining sources for every existing
    /// block. New text is permitted only for explicitly new identities.
    pub fn set_proposed_structure(
        &mut self,
        proposal_id: &str,
        placements: &[crate::ProposedBlockPlacement],
    ) -> Result<()> {
        self.atomic(|d| {
            let mut proposal = d.proposal(proposal_id)?;
            if proposal.state != ProposalState::Open {
                return Err(DocumentError::Conflict("Proposal already decided".into()));
            }
            let ProposalAction::InsertBlocks { blocks, .. } = &mut proposal.action else {
                return Err(DocumentError::Invalid(
                    "Proposal does not contain blocks".into(),
                ));
            };
            if placements.is_empty() {
                return Err(DocumentError::Invalid(
                    "Reject an empty block proposal".into(),
                ));
            }
            let existing: BTreeMap<_, _> =
                blocks.iter().map(|b| (b.id.clone(), b.clone())).collect();
            let mut next = BTreeMap::new();
            let mut positions = BTreeSet::new();
            for placement in placements {
                let block = match placement {
                    crate::ProposedBlockPlacement::Existing {
                        block,
                        parent,
                        position,
                    } => {
                        let mut current = existing.get(block).cloned().ok_or_else(|| {
                            DocumentError::NotFound {
                                kind: "proposed block",
                                id: block.clone(),
                            }
                        })?;
                        current.parent = parent.clone();
                        current.position = *position;
                        current
                    }
                    crate::ProposedBlockPlacement::New { block } => {
                        if existing.contains_key(&block.id) {
                            return Err(DocumentError::Invalid(
                                "An existing proposed block cannot be replaced with copied text"
                                    .into(),
                            ));
                        }
                        d.require_new(BLOCKS, &block.id)?;
                        Block {
                            id: block.id.clone(),
                            kind: block.kind.clone(),
                            attrs: crate::schema::normalize_attrs(
                                &block.kind,
                                block.attrs.clone(),
                            )?,
                            parent: block.parent.clone(),
                            position: block.position,
                            segments: vec![d.create_source(&block.text)?],
                            deleted: false,
                        }
                    }
                };
                if block.id.is_empty()
                    || !positions.insert((block.parent.clone(), block.position))
                    || next.insert(block.id.clone(), block).is_some()
                {
                    return Err(DocumentError::Invalid(
                        "Repeated proposed block identity or position".into(),
                    ));
                }
            }
            for block in next.values() {
                let mut seen = BTreeSet::from([&block.id]);
                let mut parent = block.parent.as_ref();
                while let Some(id) = parent {
                    if !seen.insert(id) {
                        return Err(DocumentError::Invalid("Proposed block parent cycle".into()));
                    }
                    parent = next
                        .get(id)
                        .ok_or_else(|| DocumentError::Invalid("Missing proposed parent".into()))?
                        .parent
                        .as_ref();
                }
            }
            fn append(
                parent: Option<&str>,
                source: &BTreeMap<String, Block>,
                result: &mut Vec<Block>,
            ) {
                let mut children: Vec<_> = source
                    .values()
                    .filter(|b| b.parent.as_deref() == parent)
                    .collect();
                children.sort_by_key(|b| b.position);
                for block in children {
                    result.push(block.clone());
                    append(Some(&block.id), source, result);
                }
            }
            let mut ordered = Vec::new();
            append(None, &next, &mut ordered);
            proposal.segments = ordered.iter().flat_map(|b| b.segments.clone()).collect();
            *blocks = ordered;
            d.update_review_time(&mut proposal.metadata);
            d.put_record(PROPOSALS, proposal_id, &proposal)
        })
    }

    pub fn set_proposal_details(
        &mut self,
        id: &str,
        body: &str,
        metadata: ReviewMetadata,
    ) -> Result<()> {
        self.atomic(|d| {
            let mut p = d.proposal(id)?;
            p.body = body.into();
            p.metadata = metadata;
            d.put_record(PROPOSALS, id, &p)
        })
    }

    pub fn edit_proposal(&mut self, id: &str, body: &str) -> Result<()> {
        self.atomic(|d| {
            let mut p = d.proposal(id)?;
            if p.state != ProposalState::Open {
                return Err(DocumentError::Conflict("Proposal already decided".into()));
            }
            p.body = body.into();
            d.update_review_time(&mut p.metadata);
            d.put_record(PROPOSALS, id, &p)
        })
    }

    pub fn proposal_selection(&self, id: &str, start: usize, end: usize) -> Result<Vec<TextRange>> {
        self.selection_in_segments(&self.proposal(id)?.segments, start, end)
    }
    pub fn proposal_point(&self, id: &str, offset: usize) -> Result<TextPoint> {
        self.point_in_segments(&self.proposal(id)?.segments, offset)
    }

    pub fn proposed_block_point(
        &self,
        proposal: &str,
        block: &str,
        offset: usize,
    ) -> Result<TextPoint> {
        let view = self
            .proposed_blocks_view(proposal)?
            .into_iter()
            .find(|view| view.block.id == block)
            .ok_or_else(|| DocumentError::NotFound {
                kind: "proposed block",
                id: block.into(),
            })?;
        self.point_in_segments(&view.block.segments, offset)
    }

    pub fn proposed_block_selection(
        &self,
        proposal: &str,
        block: &str,
        start: usize,
        end: usize,
    ) -> Result<Vec<TextRange>> {
        let view = self
            .proposed_blocks_view(proposal)?
            .into_iter()
            .find(|view| view.block.id == block)
            .ok_or_else(|| DocumentError::NotFound {
                kind: "proposed block",
                id: block.into(),
            })?;
        self.selection_in_segments(&view.block.segments, start, end)
    }

    pub fn proposal_target(&self, id: &str) -> Result<ResolvedTarget> {
        let proposal = self.proposal(id)?;
        match proposal.action {
            ProposalAction::Text {
                at, delete_target, ..
            } => {
                if !delete_target.is_empty() {
                    return self.resolve_target(&delete_target);
                }
                match self.locate_point(&at)? {
                    Some(point) => Ok(ResolvedTarget {
                        state: TargetState::Attached,
                        attachments: vec![crate::Attachment {
                            owner: point.owner,
                            start: point.offset,
                            end: point.offset,
                            quote: String::new(),
                        }],
                    }),
                    None => Ok(ResolvedTarget {
                        state: TargetState::Hidden,
                        attachments: Vec::new(),
                    }),
                }
            }
            ProposalAction::Format { target, .. } => self.resolve_target(&target),
            ProposalAction::DeleteBlock { block, .. }
            | ProposalAction::UpdateBlock { block, .. }
            | ProposalAction::ConvertBlock { block, .. }
            | ProposalAction::MoveBlock { block, .. } => {
                let block = self.block(&block)?;
                Ok(ResolvedTarget {
                    state: if block.deleted {
                        TargetState::Hidden
                    } else {
                        TargetState::Attached
                    },
                    attachments: if block.deleted {
                        Vec::new()
                    } else {
                        vec![crate::Attachment {
                            owner: TargetOwner::Block(block.id),
                            start: 0,
                            end: 0,
                            quote: String::new(),
                        }]
                    },
                })
            }
            ProposalAction::InsertBlocks { .. } => Ok(ResolvedTarget {
                state: TargetState::Attached,
                attachments: Vec::new(),
            }),
            ProposalAction::Unavailable { .. } => Ok(ResolvedTarget {
                state: TargetState::Unattached,
                attachments: Vec::new(),
            }),
        }
    }

    /// Read-only check shared by review projections and acceptance commands.
    /// The result is derived from current identity and content, never stored.
    pub fn validate_proposal_acceptance(&self, id: &str) -> Result<()> {
        let proposal = self.proposal(id)?;
        if proposal.state != ProposalState::Open {
            return Err(DocumentError::Conflict("Proposal already decided".into()));
        }
        let changed = |message: &str| DocumentError::Conflict(message.into());
        match &proposal.action {
            ProposalAction::Unavailable { reason } => return Err(changed(reason)),
            ProposalAction::Text {
                at,
                delete_target,
                original_quote,
            } => {
                let location = self
                    .locate_point(at)?
                    .ok_or_else(|| changed("Proposal position was deleted"))?;
                let TargetOwner::Block(block_id) = location.owner else {
                    return Err(changed(
                        "Proposed insertion no longer points to document text",
                    ));
                };
                self.active_block(&block_id)?;
                if !delete_target.is_empty() {
                    self.validate_replacement_target(delete_target, original_quote)?;
                }
            }
            ProposalAction::Format {
                target,
                name,
                expected,
                ..
            } => {
                if self.format_snapshot(target, name).as_ref().ok() != Some(expected) {
                    return Err(changed(
                        "Proposed formatting target changed; review the current text and formatting",
                    ));
                }
            }
            ProposalAction::UpdateBlock {
                block,
                block_kind: kind,
                attrs,
                expected_kind,
                expected_attrs,
            } => {
                let current = self
                    .active_block(block)
                    .map_err(|_| changed("Proposed block was removed"))?;
                if current.kind != *expected_kind || current.attrs != *expected_attrs {
                    return Err(changed(
                        "Proposed block properties changed; review the current block",
                    ));
                }
                let mut candidate = self.fork();
                candidate.set_block(block, kind, attrs.clone())?;
            }
            ProposalAction::ConvertBlock {
                block,
                target,
                expected,
            } => {
                if self.conversion_snapshot(block).as_ref().ok() != Some(expected) {
                    return Err(changed(
                        "Proposed block conversion target changed; review the current structure",
                    ));
                }
                let mut candidate = self.fork();
                candidate.convert_block(block, target)?;
            }
            ProposalAction::MoveBlock {
                block,
                parent,
                before,
                expected_parent,
                expected_before,
            } => {
                if self.block_placement(block).as_ref().ok()
                    != Some(&(expected_parent.clone(), expected_before.clone()))
                {
                    return Err(changed(
                        "Proposed block placement changed; review the current location",
                    ));
                }
                self.validate_block_move(block, parent.as_deref(), before.as_deref())?;
            }
            ProposalAction::DeleteBlock { block, expected } => {
                if self.subtree_content(block).as_ref().ok() != Some(expected) {
                    return Err(changed(
                        "Proposed block deletion target changed; create a new suggestion for the current block",
                    ));
                }
            }
            ProposalAction::InsertBlocks {
                parent,
                before,
                blocks,
            } => {
                let parent_kind = parent
                    .as_ref()
                    .map(|id| self.active_block(id).map(|b| b.kind))
                    .transpose()
                    .map_err(|_| changed("Proposed insertion destination changed"))?;
                if let Some(id) = before
                    && !self
                        .blocks()?
                        .iter()
                        .any(|b| b.id == *id && b.parent == *parent)
                {
                    return Err(changed("Proposed insertion destination changed"));
                }
                for block in blocks {
                    self.require_proposed_block_identity(block)?;
                    if block.parent.is_none() {
                        crate::schema::validate_parent(&block.kind, parent_kind.as_deref())
                            .map_err(|_| changed("Proposed insertion destination type changed"))?;
                    }
                }
            }
        }
        Ok(())
    }

    pub fn accept_proposal(&mut self, id: &str) -> Result<()> {
        self.validate_proposal_acceptance(id)?;
        self.atomic(|d| {
            let mut proposal = d.proposal(id)?;
            if proposal.state != ProposalState::Open {
                return Err(DocumentError::Conflict("Proposal already decided".into()));
            }
            match &proposal.action {
                ProposalAction::Unavailable { reason } => {
                    return Err(DocumentError::Conflict(reason.clone()));
                }
                ProposalAction::Text {
                    at, delete_target, ..
                } => {
                    let location = d.locate_point(at)?.ok_or_else(|| {
                        DocumentError::Conflict("Proposal position was deleted".into())
                    })?;
                    let TargetOwner::Block(block_id) = location.owner else {
                        return Err(DocumentError::Conflict(
                            "Proposal no longer points to document text".into(),
                        ));
                    };
                    let mut block = d.active_block(&block_id)?;
                    let mut deletes = Vec::new();
                    if !delete_target.is_empty() {
                        for fragment in delete_target {
                            let (start, end) = d.target_offsets(fragment)?;
                            deletes.push(TextRange {
                                source: fragment.source.clone(),
                                start: d.point_at(&fragment.source, start)?.cursor,
                                end: d.point_at(&fragment.source, end)?.cursor,
                            });
                        }
                    }
                    let (mut before, after) = d.partition_segments(&block.segments, at)?;
                    before.extend(proposal.segments.clone());
                    before.extend(after);
                    block.segments = before;
                    d.put_record(BLOCKS, &block.id, &block)?;
                    // Ownership is complete before running a validated text command.
                    proposal.state = ProposalState::Accepted;
                    d.put_record(PROPOSALS, id, &proposal)?;
                    d.delete_text(&deletes)?;
                }
                ProposalAction::Format {
                    target,
                    name,
                    value,
                    ..
                } => {
                    let ranges = target
                        .iter()
                        .map(|fragment| {
                            let (start, end) = d.target_offsets(fragment)?;
                            Ok(TextRange {
                                source: fragment.source.clone(),
                                start: d.point_at(&fragment.source, start)?.cursor,
                                end: d.point_at(&fragment.source, end)?.cursor,
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                    d.format(&ranges, name, value)?;
                }
                ProposalAction::UpdateBlock {
                    block,
                    block_kind: kind,
                    attrs,
                    ..
                } => {
                    d.set_block(block, kind, attrs.clone())?;
                }
                ProposalAction::ConvertBlock { block, target, .. } => {
                    d.convert_block(block, target)?;
                }
                ProposalAction::MoveBlock {
                    block,
                    parent,
                    before,
                    ..
                } => {
                    d.move_block(block, parent.clone(), before.as_deref())?;
                }
                ProposalAction::DeleteBlock { block, expected } => {
                    if d.subtree_content(block)? != *expected {
                        return Err(DocumentError::Conflict(
                            "Proposed block deletion target changed; review it again".into(),
                        ));
                    }
                    d.delete_block(block)?;
                }
                ProposalAction::InsertBlocks {
                    parent,
                    before,
                    blocks,
                } => {
                    if let Some(parent) = parent {
                        d.active_block(parent)?;
                    }
                    let mut siblings: Vec<_> = d
                        .blocks()?
                        .into_iter()
                        .filter(|b| b.parent == *parent)
                        .collect();
                    let index = match before {
                        Some(id) => siblings.iter().position(|b| b.id == *id).ok_or_else(|| {
                            DocumentError::Conflict("Proposed insertion destination changed".into())
                        })?,
                        None => siblings.len(),
                    };
                    let mut roots: Vec<_> = blocks
                        .iter()
                        .filter(|b| b.parent.is_none())
                        .cloned()
                        .collect();
                    roots.sort_by(|a, b| (a.position, &a.id).cmp(&(b.position, &b.id)));
                    for root in &mut roots {
                        root.parent = parent.clone();
                    }
                    siblings.splice(index..index, roots);
                    for block in blocks {
                        d.require_proposed_block_identity(block)?;
                        d.put_record(BLOCKS, &block.id, block)?;
                    }
                    for (position, mut block) in siblings.into_iter().enumerate() {
                        block.position = position;
                        d.put_record(BLOCKS, &block.id, &block)?;
                    }
                }
            }
            proposal.state = ProposalState::Accepted;
            d.update_review_time(&mut proposal.metadata);
            d.put_record(PROPOSALS, id, &proposal)
        })
    }

    pub fn reject_proposal(&mut self, id: &str) -> Result<()> {
        self.atomic(|d| {
            let mut proposal = d.proposal(id)?;
            if proposal.state != ProposalState::Open {
                return Err(DocumentError::Conflict("Proposal already decided".into()));
            }
            proposal.state = ProposalState::Rejected;
            d.update_review_time(&mut proposal.metadata);
            d.put_record(PROPOSALS, id, &proposal)
        })
    }

    // Undo retains inserted blocks as tombstones so their character identity
    // remains available to comments and redo. Only this proposal's exact
    // retained identity may be used again; an unrelated ID collision still
    // invalidates acceptance, even if that unrelated block was deleted.
    fn require_proposed_block_identity(&self, proposed: &Block) -> Result<()> {
        match self.block(&proposed.id) {
            Err(DocumentError::NotFound { .. }) => Ok(()),
            Ok(existing)
                if existing.deleted
                    && existing.segments == proposed.segments
                    && existing.kind == proposed.kind
                    && existing.attrs == proposed.attrs =>
            {
                Ok(())
            }
            Ok(_) => Err(DocumentError::Conflict(format!(
                "Proposed block ID is already in use: {}",
                proposed.id
            ))),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn subtree_content(&self, root: &str) -> Result<String> {
        self.active_block(root)?;
        let blocks = self.blocks()?;
        let mut ids = BTreeSet::from([root.to_string()]);
        loop {
            let count = ids.len();
            for b in &blocks {
                if b.parent.as_ref().is_some_and(|p| ids.contains(p)) {
                    ids.insert(b.id.clone());
                }
            }
            if ids.len() == count {
                break;
            }
        }
        let mut content = Vec::new();
        for id in ids {
            let view = self.block_view(&id)?;
            let mut runs: Vec<(
                String,
                std::collections::BTreeMap<String, serde_json::Value>,
            )> = Vec::new();
            for run in view.runs {
                if let Some(last) = runs.last_mut().filter(|r| r.1 == run.marks) {
                    last.0.push_str(&run.text);
                } else {
                    runs.push((run.text, run.marks));
                }
            }
            content.push(serde_json::json!({"id":id,"kind":view.block.kind,"attrs":view.block.attrs,"runs":runs,
                "parent":if id==root {None}else{view.block.parent},"position":if id==root {0}else{view.block.position}}));
        }
        Ok(serde_json::to_string(&content)?)
    }
}
