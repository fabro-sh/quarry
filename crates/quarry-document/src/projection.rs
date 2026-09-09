use crate::{
    Attachment, BlockView, Document, DocumentError, Result, SegmentRef, TargetOwner, TextPoint,
    TextRange, TextRun, byte_offset, text_slice, utf16_len,
};
use automerge::{ReadDoc, ScalarValue, iter::Span};

#[derive(Clone, Debug)]
pub(crate) struct Segment {
    pub reference: SegmentRef,
    /// Start immediately after the marker; end immediately before the next.
    pub start: usize,
    pub end: usize,
    pub runs: Vec<TextRun>,
}

impl Document {
    pub fn view(&self) -> Result<crate::DocumentView> {
        Ok(crate::DocumentView {
            document_id: self.id()?,
            schema_version: crate::SCHEMA_VERSION,
            conflicts: self.conflicts()?,
            heads: self.heads().iter().map(ToString::to_string).collect(),
            blocks: self
                .blocks()?
                .into_iter()
                .map(|b| self.view_block(b))
                .collect::<Result<_>>()?,
            comments: self
                .comments()?
                .into_iter()
                .map(|comment| {
                    Ok(crate::CommentView {
                        target: self.comment_target(&comment.id)?,
                        comment,
                    })
                })
                .collect::<Result<_>>()?,
            proposals: self
                .proposals()?
                .into_iter()
                .map(|proposal| self.view_proposal(proposal))
                .collect::<Result<_>>()?,
        })
    }

    /// Project one proposal without serializing unrelated document blocks or
    /// review items. Editors use this for conformance checks on each input.
    pub fn proposal_view(&self, id: &str) -> Result<crate::ProposalView> {
        self.view_proposal(self.proposal(id)?)
    }

    /// Project review proposals attached to one canonical block without
    /// materializing unrelated blocks, comments, or conflicts.
    pub fn proposal_views_for_block(&self, block: &str) -> Result<Vec<crate::ProposalView>> {
        self.proposals()?
            .into_iter()
            .map(|proposal| self.view_proposal(proposal))
            .filter(|view| {
                view.as_ref().map_or(true, |view| {
                    view.target.attachments.iter().any(
                        |part| matches!(&part.owner, crate::TargetOwner::Block(id) if id == block),
                    )
                })
            })
            .collect()
    }

    /// Project the visible review markers for one displayed text owner. This
    /// lets an editor update a changed block without serializing the document.
    pub fn review_markers(
        &self,
        owner: &crate::TargetOwner,
    ) -> Result<Vec<(String, String, usize, usize)>> {
        let mut markers = Vec::new();
        for comment in self.comments()? {
            if comment.deleted
                || comment.parent_id.is_some()
                || comment.state != crate::DiscussionState::Open
            {
                continue;
            }
            for part in self.comment_target(&comment.id)?.attachments {
                if &part.owner == owner {
                    markers.push(("comment".into(), comment.id.clone(), part.start, part.end));
                }
            }
        }
        for proposal in self.proposals()? {
            if proposal.state != crate::ProposalState::Open {
                continue;
            }
            let kind = match &proposal.action {
                crate::ProposalAction::Unavailable { .. } => "unavailable",
                crate::ProposalAction::Text { .. } => "text",
                crate::ProposalAction::Format { .. } => "format",
                crate::ProposalAction::UpdateBlock { .. } => "update_block",
                crate::ProposalAction::ConvertBlock { .. } => "convert_block",
                crate::ProposalAction::MoveBlock { .. } => "move_block",
                crate::ProposalAction::SplitBlock { .. } => "split_block",
                crate::ProposalAction::PasteBlocks { .. } => "paste_blocks",
                crate::ProposalAction::JoinBlocks { .. } => "join_blocks",
                crate::ProposalAction::DeleteBlock { .. } => "delete_block",
                crate::ProposalAction::InsertBlocks { .. } => "insert_blocks",
            };
            for part in self.proposal_target(&proposal.id)?.attachments {
                if &part.owner == owner {
                    markers.push((kind.into(), proposal.id.clone(), part.start, part.end));
                }
            }
        }
        Ok(markers)
    }

    fn view_proposal(&self, proposal: crate::Proposal) -> Result<crate::ProposalView> {
        let mut runs = Vec::new();
        for reference in &proposal.segments {
            runs.extend(self.segment(reference)?.runs);
        }
        Ok(crate::ProposalView {
            blocks: self.proposed_blocks_view(&proposal.id)?,
            acceptance_error: self
                .validate_proposal_acceptance(&proposal.id)
                .err()
                .map(|error| error.to_string()),
            target: self.proposal_target(&proposal.id)?,
            text: runs.iter().map(|run| run.text.as_str()).collect(),
            runs,
            proposal,
        })
    }

    /// Map a saved browser selection back to the current displayed owner.
    /// A deleted target returns None; repeated text is never searched.
    pub fn locate_point(&self, point: &TextPoint) -> Result<Option<crate::ResolvedPoint>> {
        let position = self.resolve_point(point)?;
        let blocks = self.cached_blocks()?;
        let proposals = self.proposals()?;
        let owners = blocks
            .ordered
            .iter()
            .map(|b| (false, &b.id, &b.segments))
            .chain(
                proposals
                    .iter()
                    .filter(|p| p.state == crate::ProposalState::Open)
                    .map(|p| (true, &p.id, &p.segments)),
            );
        for (proposal, id, references) in owners {
            if !references
                .iter()
                .any(|reference| reference.source == point.source)
            {
                continue;
            }
            let mut offset = 0;
            for reference in references {
                let segment = self.segment(reference)?;
                if reference.source == point.source
                    && segment.start <= position
                    && position <= segment.end
                {
                    let within_segment = segment
                        .runs
                        .iter()
                        .map(|r| (position.saturating_sub(r.source_start)).min(utf16_len(&r.text)))
                        .sum::<usize>();
                    let mut proposed_block = None;
                    if proposal
                        && let Some(crate::ProposalAction::InsertBlocks { blocks, .. }) =
                            proposals.iter().find(|p| &p.id == id).map(|p| &p.action)
                    {
                        'blocks: for block in blocks {
                            let mut preceding = 0;
                            for part in &block.segments {
                                if part == reference {
                                    proposed_block = Some(crate::ResolvedProposedPoint {
                                        block: block.id.clone(),
                                        offset: preceding + within_segment,
                                    });
                                    break 'blocks;
                                }
                                preceding += self
                                    .segment(part)?
                                    .runs
                                    .iter()
                                    .map(|r| utf16_len(&r.text))
                                    .sum::<usize>();
                            }
                        }
                    }
                    return Ok(Some(crate::ResolvedPoint {
                        owner: if proposal {
                            TargetOwner::Proposal(id.clone())
                        } else {
                            TargetOwner::Block(id.clone())
                        },
                        offset: offset + within_segment,
                        proposed_block,
                    }));
                }
                offset += segment
                    .runs
                    .iter()
                    .map(|r| utf16_len(&r.text))
                    .sum::<usize>();
            }
        }
        Ok(None)
    }

    /// Derived spans are shared by forks. Only text mutations invalidate their
    /// source; a history merge clears the cache. Validation always visits every
    /// source and record, including unchanged sources served by this cache.
    pub(crate) fn invalidate_source(&mut self, source: &str) {
        self.segments
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(source);
    }

    pub(crate) fn source_segments(&self, source: &str) -> Result<std::sync::Arc<Vec<Segment>>> {
        if let Some(segments) = self
            .segments
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(source)
        {
            return Ok(std::sync::Arc::clone(segments));
        }
        let segments = std::sync::Arc::new(self.read_source_segments(source)?);
        self.segments
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(source.into(), std::sync::Arc::clone(&segments));
        Ok(segments)
    }

    fn read_source_segments(&self, source: &str) -> Result<Vec<Segment>> {
        let mut segments: Vec<Segment> = Vec::new();
        let mut index = 0;
        for span in self.crdt.spans(self.source(source)?)? {
            match span {
                Span::Block(marker) => {
                    if let Some(last) = segments.last_mut() {
                        last.end = index;
                    }
                    let Some(automerge::hydrate::Value::Scalar(ScalarValue::Str(id))) =
                        marker.get("segment")
                    else {
                        return Err(DocumentError::Invalid(
                            "Text marker has no segment ID".into(),
                        ));
                    };
                    index += 1;
                    segments.push(Segment {
                        reference: SegmentRef {
                            source: source.into(),
                            segment: id.to_string(),
                        },
                        start: index,
                        end: index,
                        runs: Vec::new(),
                    });
                }
                Span::Text { text, marks } => {
                    let Some(segment) = segments.last_mut() else {
                        return Err(DocumentError::Invalid(
                            "Text precedes the first segment marker".into(),
                        ));
                    };
                    let deleted: Vec<_> = marks
                        .as_ref()
                        .map(|m| {
                            m.iter()
                                .filter_map(|(name, value)| {
                                    if !name.starts_with("quarry:deleted:") {
                                        return None;
                                    }
                                    match value {
                                        ScalarValue::Str(cursor) => Some(cursor.to_string()),
                                        _ => None,
                                    }
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let formatting: std::collections::BTreeMap<_, _> = marks
                        .map(|m| {
                            m.iter()
                                .filter(|(name, _)| !name.starts_with("quarry:"))
                                .map(|(name, value)| (name.to_string(), scalar_json(value)))
                                .collect()
                        })
                        .unwrap_or_default();
                    for (name, value) in &formatting {
                        crate::schema::validate_format(name, value)?;
                    }
                    if deleted.is_empty() {
                        let run = TextRun {
                            text,
                            marks: formatting,
                            source: source.into(),
                            source_start: index,
                        };
                        index += utf16_len(&run.text);
                        segment.runs.push(run);
                    } else {
                        for ch in text.chars() {
                            let cursor = self.point_at(source, index)?.cursor;
                            if !deleted.contains(&cursor) {
                                if let Some(last) = segment.runs.last_mut().filter(|r| {
                                    r.source_start + utf16_len(&r.text) == index
                                        && r.marks == formatting
                                }) {
                                    last.text.push(ch);
                                } else {
                                    segment.runs.push(TextRun {
                                        text: ch.to_string(),
                                        marks: formatting.clone(),
                                        source: source.into(),
                                        source_start: index,
                                    });
                                }
                            }
                            index += ch.len_utf16();
                        }
                    }
                    segment.end = index;
                }
            }
        }
        Ok(segments)
    }

    pub(crate) fn segment(&self, reference: &SegmentRef) -> Result<Segment> {
        self.source_segments(&reference.source)?
            .iter()
            .find(|s| s.reference == *reference)
            .cloned()
            .ok_or_else(|| DocumentError::NotFound {
                kind: "segment",
                id: reference.segment.clone(),
            })
    }

    pub fn block_view(&self, id: &str) -> Result<BlockView> {
        let block = self.block(id)?;
        self.view_block(block)
    }

    pub fn proposed_blocks_view(&self, id: &str) -> Result<Vec<BlockView>> {
        match self.proposal(id)?.action {
            crate::ProposalAction::InsertBlocks { blocks, .. }
            | crate::ProposalAction::PasteBlocks { blocks, .. } => {
                blocks.into_iter().map(|b| self.view_block(b)).collect()
            }
            _ => Ok(Vec::new()),
        }
    }

    fn view_block(&self, block: crate::Block) -> Result<BlockView> {
        let mut runs = Vec::new();
        for reference in &block.segments {
            runs.extend(self.segment(reference)?.runs);
        }
        let text = runs.iter().map(|r| r.text.as_str()).collect();
        Ok(BlockView { block, text, runs })
    }

    /// Capture a native address while the caller's document version is current.
    pub fn point(&self, block: &str, offset: usize) -> Result<TextPoint> {
        self.point_in_segments(&self.block(block)?.segments, offset)
    }

    pub(crate) fn point_in_segments(
        &self,
        references: &[SegmentRef],
        offset: usize,
    ) -> Result<TextPoint> {
        let mut remaining = offset;
        for (index, reference) in references.iter().enumerate() {
            let segment = self.segment(reference)?;
            let len: usize = segment.runs.iter().map(|r| utf16_len(&r.text)).sum();
            if remaining < len || (remaining == len && index + 1 == references.len()) {
                let mut within = remaining;
                for run in &segment.runs {
                    let length = utf16_len(&run.text);
                    if within < length {
                        byte_offset(&run.text, within)?;
                        return self.point_at(&reference.source, run.source_start + within);
                    }
                    within -= length;
                }
                return self.point_at(&reference.source, segment.end);
            }
            remaining = remaining
                .checked_sub(len)
                .ok_or(DocumentError::InvalidOffset(offset))?;
        }
        Err(DocumentError::InvalidOffset(offset))
    }

    pub fn selection(&self, block: &str, start: usize, end: usize) -> Result<Vec<TextRange>> {
        self.selection_in_segments(&self.block(block)?.segments, start, end)
    }

    pub(crate) fn selection_in_segments(
        &self,
        references: &[SegmentRef],
        start: usize,
        end: usize,
    ) -> Result<Vec<TextRange>> {
        if start > end {
            return Err(DocumentError::InvalidOffset(end));
        }
        self.point_in_segments(references, start)?;
        self.point_in_segments(references, end)?;
        let mut ranges: Vec<TextRange> = Vec::new();
        let mut offset = 0;
        for reference in references {
            for run in self.segment(reference)?.runs {
                let length = utf16_len(&run.text);
                let from = start.max(offset);
                let to = end.min(offset + length);
                if from < to {
                    let range = TextRange {
                        source: reference.source.clone(),
                        start: self
                            .point_at(&reference.source, run.source_start + from - offset)?
                            .cursor,
                        end: self
                            .point_at(&reference.source, run.source_start + to - offset)?
                            .cursor,
                    };
                    if let Some(previous) = ranges.last_mut().filter(|previous| {
                        previous.source == range.source && previous.end == range.start
                    }) {
                        previous.end = range.end;
                    } else {
                        ranges.push(range);
                    }
                }
                offset += length;
            }
        }
        Ok(ranges)
    }

    pub(crate) fn attach_range(
        &self,
        owner: TargetOwner,
        refs: &[SegmentRef],
        source: &str,
        start: usize,
        end: usize,
    ) -> Result<Vec<Attachment>> {
        let mut attachments = Vec::new();
        let mut offset = 0;
        for reference in refs {
            for run in self.segment(reference)?.runs {
                let length = utf16_len(&run.text);
                if reference.source == source {
                    let from = start.max(run.source_start);
                    let to = end.min(run.source_start + length);
                    if from < to {
                        attachments.push(Attachment {
                            owner: owner.clone(),
                            start: offset + from - run.source_start,
                            end: offset + to - run.source_start,
                            quote: text_slice(
                                &run.text,
                                from - run.source_start,
                                to - run.source_start,
                            )?
                            .into(),
                        });
                    }
                }
                offset += length;
            }
        }
        Ok(attachments)
    }
}

fn scalar_json(value: &ScalarValue) -> serde_json::Value {
    match value {
        ScalarValue::Str(s) => serde_json::Value::String(s.to_string()),
        ScalarValue::Boolean(v) => (*v).into(),
        ScalarValue::Int(v) => (*v).into(),
        ScalarValue::Uint(v) => (*v).into(),
        ScalarValue::F64(v) => serde_json::json!(v),
        _ => serde_json::Value::Null,
    }
}
